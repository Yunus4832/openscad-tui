use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::{Aabb, Camera, Mesh, MeshInput, MeshLoader, PixelSize, RenderError, Result, RgbaFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    pub axes: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { axes: true }
    }
}

pub trait FrameRenderer: Send + Sync {
    fn render_frame(
        &self,
        mesh: &Mesh,
        camera: &Camera,
        size: PixelSize,
        options: RenderOptions,
    ) -> Result<RgbaFrame>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderFailureStage {
    Loading,
    Rasterizing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedFrame {
    pub mesh_revision: u64,
    pub camera_revision: u64,
    pub frame: RgbaFrame,
    pub triangle_count: usize,
    pub bounds: Aabb,
    pub generation_time: Duration,
    pub raster_time: Duration,
}

#[derive(Debug, PartialEq)]
pub enum RenderEvent {
    Loading {
        mesh_revision: u64,
    },
    Rasterizing {
        mesh_revision: u64,
        camera_revision: u64,
    },
    Ready(RenderedFrame),
    Failed {
        mesh_revision: u64,
        camera_revision: u64,
        stage: RenderFailureStage,
        error: RenderError,
    },
}

enum RenderRequest {
    Load {
        mesh_revision: u64,
        camera_revision: u64,
        input: MeshInput,
        camera: Camera,
        size: PixelSize,
        options: RenderOptions,
    },
    Rasterize {
        camera_revision: u64,
        camera: Camera,
        size: PixelSize,
        options: RenderOptions,
    },
    Shutdown,
}

pub struct RenderService {
    requests: Sender<RenderRequest>,
    events: Receiver<RenderEvent>,
    worker: Option<JoinHandle<()>>,
}

impl RenderService {
    pub fn new(loader: Box<dyn MeshLoader>, renderer: Box<dyn FrameRenderer>) -> Self {
        let (request_sender, request_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("openscad-render".to_string())
            .spawn(move || worker_loop(request_receiver, event_sender, loader, renderer))
            .expect("failed to start render worker");
        Self {
            requests: request_sender,
            events: event_receiver,
            worker: Some(worker),
        }
    }

    pub fn load(
        &self,
        mesh_revision: u64,
        camera_revision: u64,
        input: MeshInput,
        camera: Camera,
        size: PixelSize,
        options: RenderOptions,
    ) -> Result<()> {
        self.requests
            .send(RenderRequest::Load {
                mesh_revision,
                camera_revision,
                input,
                camera,
                size,
                options,
            })
            .map_err(|_| RenderError::WorkerDisconnected)
    }

    pub fn rasterize(
        &self,
        camera_revision: u64,
        camera: Camera,
        size: PixelSize,
        options: RenderOptions,
    ) -> Result<()> {
        self.requests
            .send(RenderRequest::Rasterize {
                camera_revision,
                camera,
                size,
                options,
            })
            .map_err(|_| RenderError::WorkerDisconnected)
    }

    pub fn try_recv(&self) -> Option<RenderEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for RenderService {
    fn drop(&mut self) {
        let _ = self.requests.send(RenderRequest::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(
    requests: Receiver<RenderRequest>,
    events: Sender<RenderEvent>,
    loader: Box<dyn MeshLoader>,
    renderer: Box<dyn FrameRenderer>,
) {
    let mut cached_mesh: Option<(u64, Mesh, Duration)> = None;
    let mut pending = None;
    loop {
        let first = match pending.take().map(Ok).unwrap_or_else(|| requests.recv()) {
            Ok(request) => request,
            Err(_) => return,
        };
        let Some(work) = coalesce(first, &requests) else {
            return;
        };

        let (mesh_revision, camera_revision, camera, size, options) = match work {
            RenderRequest::Load {
                mesh_revision,
                camera_revision,
                input,
                camera,
                size,
                options,
            } => {
                let _ = events.send(RenderEvent::Loading { mesh_revision });
                match loader.load(&input) {
                    Ok(generation) => {
                        cached_mesh = Some((
                            mesh_revision,
                            generation.mesh,
                            generation.diagnostics.elapsed,
                        ))
                    }
                    Err(error) => {
                        let _ = events.send(RenderEvent::Failed {
                            mesh_revision,
                            camera_revision,
                            stage: RenderFailureStage::Loading,
                            error,
                        });
                        continue;
                    }
                }
                (mesh_revision, camera_revision, camera, size, options)
            }
            RenderRequest::Rasterize {
                camera_revision,
                camera,
                size,
                options,
            } => {
                let Some((mesh_revision, _, _)) = cached_mesh.as_ref() else {
                    let _ = events.send(RenderEvent::Failed {
                        mesh_revision: 0,
                        camera_revision,
                        stage: RenderFailureStage::Rasterizing,
                        error: RenderError::NoCachedMesh,
                    });
                    continue;
                };
                (*mesh_revision, camera_revision, camera, size, options)
            }
            RenderRequest::Shutdown => return,
        };

        // A model request received while OpenSCAD was running supersedes this mesh before it is
        // rasterized. Camera-only requests are merged into the pending work below as well.
        match drain_latest(&requests) {
            DrainResult::Shutdown => return,
            DrainResult::Pending(request) => {
                pending = Some(request);
                continue;
            }
            DrainResult::Empty => {}
        }

        let Some((_, mesh, generation_time)) = &cached_mesh else {
            continue;
        };
        let _ = events.send(RenderEvent::Rasterizing {
            mesh_revision,
            camera_revision,
        });
        let raster_started = Instant::now();
        let rendered = renderer.render_frame(mesh, &camera, size, options);
        let raster_time = raster_started.elapsed();

        // Preserve the newest pending request, but still publish this completed camera frame.
        // Continuous input otherwise starves presentation: every finished frame would be dropped
        // merely because the mouse moved again while it was being rasterized.
        let next = drain_latest(&requests);
        if matches!(next, DrainResult::Shutdown) {
            return;
        }

        match rendered {
            Ok(frame) => {
                let _ = events.send(RenderEvent::Ready(RenderedFrame {
                    mesh_revision,
                    camera_revision,
                    frame,
                    triangle_count: mesh.triangle_count(),
                    bounds: mesh.bounds,
                    generation_time: *generation_time,
                    raster_time,
                }));
            }
            Err(error) => {
                let _ = events.send(RenderEvent::Failed {
                    mesh_revision,
                    camera_revision,
                    stage: RenderFailureStage::Rasterizing,
                    error,
                });
            }
        }
        if let DrainResult::Pending(request) = next {
            pending = Some(request);
        }
    }
}

fn coalesce(first: RenderRequest, requests: &Receiver<RenderRequest>) -> Option<RenderRequest> {
    let mut latest = first;
    loop {
        match requests.try_recv() {
            Ok(RenderRequest::Shutdown) => return None,
            Ok(request) => latest = merge(latest, request),
            Err(TryRecvError::Empty) => return Some(latest),
            Err(TryRecvError::Disconnected) => return None,
        }
    }
}

fn merge(current: RenderRequest, next: RenderRequest) -> RenderRequest {
    match (current, next) {
        (
            RenderRequest::Load {
                mesh_revision,
                input,
                ..
            },
            RenderRequest::Rasterize {
                camera_revision,
                camera,
                size,
                options,
            },
        ) => RenderRequest::Load {
            mesh_revision,
            camera_revision,
            input,
            camera,
            size,
            options,
        },
        (_, next) => next,
    }
}

enum DrainResult {
    Empty,
    Pending(RenderRequest),
    Shutdown,
}

fn drain_latest(requests: &Receiver<RenderRequest>) -> DrainResult {
    match requests.try_recv() {
        Ok(RenderRequest::Shutdown) => DrainResult::Shutdown,
        Ok(request) => match coalesce(request, requests) {
            Some(request) => DrainResult::Pending(request),
            None => DrainResult::Shutdown,
        },
        Err(TryRecvError::Empty) => DrainResult::Empty,
        Err(TryRecvError::Disconnected) => DrainResult::Shutdown,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::{GenerationDiagnostics, MeshGeneration, Vec3};

    struct FakeLoader {
        calls: Arc<AtomicUsize>,
        delay: Duration,
    }

    impl MeshLoader for FakeLoader {
        fn load(&self, _input: &MeshInput) -> Result<MeshGeneration> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            thread::sleep(self.delay);
            Ok(MeshGeneration {
                mesh: Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]])?,
                diagnostics: GenerationDiagnostics {
                    stdout: String::new(),
                    stderr: String::new(),
                    elapsed: self.delay,
                },
            })
        }
    }

    struct FakeRenderer {
        calls: Arc<AtomicUsize>,
        delay: Duration,
    }

    impl FrameRenderer for FakeRenderer {
        fn render_frame(
            &self,
            _mesh: &Mesh,
            _camera: &Camera,
            size: PixelSize,
            _options: RenderOptions,
        ) -> Result<RgbaFrame> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            thread::sleep(self.delay);
            Ok(RgbaFrame::new(size, [1, 2, 3, 255]))
        }
    }

    fn service(
        loader_calls: Arc<AtomicUsize>,
        renderer_calls: Arc<AtomicUsize>,
        delay: Duration,
    ) -> RenderService {
        RenderService::new(
            Box::new(FakeLoader {
                calls: loader_calls,
                delay,
            }),
            Box::new(FakeRenderer {
                calls: renderer_calls,
                delay,
            }),
        )
    }

    fn wait_ready(service: &RenderService) -> RenderedFrame {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if let Some(RenderEvent::Ready(frame)) = service.try_recv() {
                return frame;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("render service did not produce a frame");
    }

    #[test]
    fn camera_render_reuses_the_generated_mesh() {
        let loader_calls = Arc::new(AtomicUsize::new(0));
        let renderer_calls = Arc::new(AtomicUsize::new(0));
        let service = service(loader_calls.clone(), renderer_calls.clone(), Duration::ZERO);
        let size = PixelSize::new(8, 8).unwrap();
        service
            .load(
                4,
                1,
                MeshInput::OpenScad("cube(1);".to_string()),
                Camera::default(),
                size,
                RenderOptions::default(),
            )
            .unwrap();
        assert_eq!(wait_ready(&service).mesh_revision, 4);
        service
            .rasterize(2, Camera::default(), size, RenderOptions::default())
            .unwrap();
        let frame = wait_ready(&service);
        assert_eq!(frame.camera_revision, 2);
        assert_eq!(loader_calls.load(Ordering::SeqCst), 1);
        assert_eq!(renderer_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn camera_requests_are_latest_wins() {
        let loader_calls = Arc::new(AtomicUsize::new(0));
        let renderer_calls = Arc::new(AtomicUsize::new(0));
        let service = service(loader_calls, renderer_calls, Duration::from_millis(20));
        let size = PixelSize::new(8, 8).unwrap();
        service
            .load(
                1,
                1,
                MeshInput::OpenScad("cube(1);".to_string()),
                Camera::default(),
                size,
                RenderOptions::default(),
            )
            .unwrap();
        service
            .rasterize(2, Camera::default(), size, RenderOptions::default())
            .unwrap();
        service
            .rasterize(3, Camera::default(), size, RenderOptions::default())
            .unwrap();
        let frame = wait_ready(&service);
        assert_eq!(frame.mesh_revision, 1);
        assert_eq!(frame.camera_revision, 3);
    }

    #[test]
    fn completed_camera_frame_is_published_before_latest_pending_frame() {
        let service = service(
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Duration::from_millis(20),
        );
        let size = PixelSize::new(8, 8).unwrap();
        service
            .load(
                1,
                1,
                MeshInput::OpenScad("cube(1);".to_string()),
                Camera::default(),
                size,
                RenderOptions::default(),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut started = false;
        while std::time::Instant::now() < deadline {
            if matches!(service.try_recv(), Some(RenderEvent::Rasterizing { .. })) {
                started = true;
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(started, "first camera frame did not start rasterizing");
        service
            .rasterize(2, Camera::default(), size, RenderOptions::default())
            .unwrap();

        assert_eq!(wait_ready(&service).camera_revision, 1);
        assert_eq!(wait_ready(&service).camera_revision, 2);
    }

    #[test]
    fn rasterize_without_mesh_reports_failure() {
        let service = service(
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Duration::ZERO,
        );
        service
            .rasterize(
                1,
                Camera::default(),
                PixelSize::new(8, 8).unwrap(),
                RenderOptions::default(),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            if let Some(RenderEvent::Failed { error, .. }) = service.try_recv() {
                assert_eq!(error, RenderError::NoCachedMesh);
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("expected failure event");
    }

    #[test]
    fn coalescing_keeps_the_latest_render_options() {
        let merged = merge(
            RenderRequest::Load {
                mesh_revision: 4,
                camera_revision: 1,
                input: MeshInput::OpenScad("cube(1);".to_string()),
                camera: Camera::default(),
                size: PixelSize::new(8, 8).unwrap(),
                options: RenderOptions { axes: true },
            },
            RenderRequest::Rasterize {
                camera_revision: 2,
                camera: Camera::default(),
                size: PixelSize::new(16, 16).unwrap(),
                options: RenderOptions { axes: false },
            },
        );

        assert!(matches!(
            merged,
            RenderRequest::Load {
                camera_revision: 2,
                options: RenderOptions { axes: false },
                ..
            }
        ));
    }
}
