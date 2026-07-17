use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use crate::{Aabb, Mat4, Vec3};

const MIN_DISTANCE: f32 = 1.0e-4;
const MIN_SCALE: f32 = 1.0e-4;
const MAX_PITCH: f32 = FRAC_PI_2 - 1.0e-3;
const FIT_MARGIN: f32 = 1.15;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    Perspective { fov_y_radians: f32 },
    Orthographic { vertical_size: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Isometric,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub projection: Projection,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 5.0,
            yaw: -FRAC_PI_2,
            pitch: FRAC_PI_4 * 0.5,
            projection: Projection::Perspective {
                fov_y_radians: 45.0_f32.to_radians(),
            },
            near: 0.01,
            far: 1000.0,
        }
    }
}

impl Camera {
    pub fn position(self) -> Vec3 {
        let cos_pitch = self.pitch.cos();
        self.target
            + Vec3::new(
                cos_pitch * self.yaw.cos(),
                cos_pitch * self.yaw.sin(),
                self.pitch.sin(),
            ) * self.distance
    }

    pub fn view_matrix(self) -> Mat4 {
        Mat4::look_at_rh(self.position(), self.target, self.camera_up())
    }

    pub fn projection_matrix(self, aspect_ratio: f32) -> Mat4 {
        let aspect = aspect_ratio.max(1.0e-4);
        match self.projection {
            Projection::Perspective { fov_y_radians } => {
                Mat4::perspective_rh_gl(fov_y_radians, aspect, self.near, self.far)
            }
            Projection::Orthographic { vertical_size } => {
                let half_height = vertical_size.max(MIN_SCALE) * 0.5;
                let half_width = half_height * aspect;
                Mat4::orthographic_rh_gl(
                    -half_width,
                    half_width,
                    -half_height,
                    half_height,
                    self.near,
                    self.far,
                )
            }
        }
    }

    pub fn view_projection(self, aspect_ratio: f32) -> Mat4 {
        self.projection_matrix(aspect_ratio) * self.view_matrix()
    }

    pub fn orbit(&mut self, delta_yaw: f32, delta_pitch: f32) {
        self.yaw = (self.yaw + delta_yaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + delta_pitch).clamp(-MAX_PITCH, MAX_PITCH);
    }

    pub fn pan(&mut self, horizontal: f32, vertical: f32) {
        let forward = (self.target - self.position()).normalize_or_zero();
        let right = forward.cross(self.camera_up()).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        self.target += right * horizontal + up * vertical;
    }

    pub fn pan_fraction(&mut self, horizontal: f32, vertical: f32) {
        let visible_height = match self.projection {
            Projection::Perspective { fov_y_radians } => {
                2.0 * self.distance * (fov_y_radians * 0.5).tan()
            }
            Projection::Orthographic { vertical_size } => vertical_size,
        };
        self.pan(horizontal * visible_height, vertical * visible_height);
    }

    pub fn zoom(&mut self, factor: f32) {
        let factor = factor.max(1.0e-4);
        match &mut self.projection {
            Projection::Perspective { .. } => {
                self.distance = (self.distance * factor).max(MIN_DISTANCE);
            }
            Projection::Orthographic { vertical_size } => {
                *vertical_size = (*vertical_size * factor).max(MIN_SCALE);
            }
        }
    }

    pub fn set_projection(&mut self, projection: Projection, bounds: Aabb, aspect_ratio: f32) {
        self.projection = projection;
        self.fit(bounds, aspect_ratio);
    }

    pub fn set_standard_view(&mut self, view: StandardView, bounds: Aabb, aspect_ratio: f32) {
        let (yaw, pitch) = match view {
            StandardView::Front => (-FRAC_PI_2, 0.0),
            StandardView::Back => (FRAC_PI_2, 0.0),
            StandardView::Left => (std::f32::consts::PI, 0.0),
            StandardView::Right => (0.0, 0.0),
            StandardView::Top => (-FRAC_PI_2, MAX_PITCH),
            StandardView::Bottom => (-FRAC_PI_2, -MAX_PITCH),
            StandardView::Isometric => (-FRAC_PI_4, 35.264_f32.to_radians()),
        };
        self.yaw = yaw;
        self.pitch = pitch;
        self.fit(bounds, aspect_ratio);
    }

    pub fn fit(&mut self, bounds: Aabb, aspect_ratio: f32) {
        let radius = bounds.radius().max(MIN_DISTANCE);
        let aspect = aspect_ratio.max(1.0e-4);
        self.target = bounds.center();
        match &mut self.projection {
            Projection::Perspective { fov_y_radians } => {
                let vertical_fov =
                    (*fov_y_radians).clamp(1.0_f32.to_radians(), 179.0_f32.to_radians());
                let horizontal_fov = 2.0 * ((vertical_fov * 0.5).tan() * aspect).atan();
                let limiting_fov = vertical_fov.min(horizontal_fov);
                self.distance = radius / (limiting_fov * 0.5).sin() * FIT_MARGIN;
            }
            Projection::Orthographic { vertical_size } => {
                *vertical_size = radius * 2.0 * FIT_MARGIN / aspect.min(1.0);
                self.distance = radius * 3.0;
            }
        }
        self.near = (self.distance - radius * 1.5).max(radius * 1.0e-3);
        self.far = (self.distance + radius * 1.5).max(self.near + radius);
    }

    fn camera_up(self) -> Vec3 {
        let view_direction = (self.target - self.position()).normalize_or_zero();
        if view_direction.dot(Vec3::Z).abs() > 0.999 {
            Vec3::Y
        } else {
            Vec3::Z
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Aabb {
        Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        }
    }

    #[test]
    fn orbit_preserves_distance_and_clamps_pitch() {
        let mut camera = Camera::default();
        let distance = camera.position().distance(camera.target);
        camera.orbit(0.4, 100.0);
        assert!((camera.position().distance(camera.target) - distance).abs() < 1.0e-5);
        assert!(camera.pitch < FRAC_PI_2);
    }

    #[test]
    fn pan_moves_target_in_the_camera_plane() {
        let mut camera = Camera::default();
        let before = camera.target;
        let before_position = camera.position();
        camera.pan(1.0, 0.0);
        let movement = camera.target - before;
        let forward = (before - before_position).normalize();
        assert!(movement.length() > 0.99);
        assert!(movement.dot(forward).abs() < 1.0e-5);
    }

    #[test]
    fn zoom_updates_distance_or_orthographic_scale() {
        let mut camera = Camera::default();
        camera.zoom(0.5);
        assert_eq!(camera.distance, 2.5);

        camera.projection = Projection::Orthographic { vertical_size: 4.0 };
        camera.zoom(0.5);
        assert_eq!(
            camera.projection,
            Projection::Orthographic { vertical_size: 2.0 }
        );
    }

    #[test]
    fn fit_places_bounds_inside_clip_space_for_both_projections() {
        for projection in [
            Projection::Perspective {
                fov_y_radians: 45.0_f32.to_radians(),
            },
            Projection::Orthographic { vertical_size: 2.0 },
        ] {
            let mut camera = Camera {
                projection,
                ..Camera::default()
            };
            camera.fit(bounds(), 16.0 / 9.0);
            let matrix = camera.view_projection(16.0 / 9.0);
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        let clip = matrix * Vec3::new(x, y, z).extend(1.0);
                        let ndc = clip.truncate() / clip.w;
                        assert!(ndc.x.abs() <= 1.0);
                        assert!(ndc.y.abs() <= 1.0);
                        assert!(ndc.z.abs() <= 1.0);
                    }
                }
            }
        }
    }

    #[test]
    fn standard_views_have_expected_camera_directions() {
        let mut camera = Camera::default();
        camera.set_standard_view(StandardView::Front, bounds(), 1.0);
        assert!(camera.position().y < camera.target.y);
        camera.set_standard_view(StandardView::Right, bounds(), 1.0);
        assert!(camera.position().x > camera.target.x);
        camera.set_standard_view(StandardView::Top, bounds(), 1.0);
        assert!(camera.position().z > camera.target.z);
    }

    #[test]
    fn orthographic_projection_preserves_size_with_depth() {
        let camera = Camera {
            projection: Projection::Orthographic { vertical_size: 4.0 },
            ..Camera::default()
        };
        let matrix = camera.view_projection(1.0);
        let near_a = matrix * Vec3::new(0.0, 0.0, 0.0).extend(1.0);
        let near_b = matrix * Vec3::new(1.0, 0.0, 0.0).extend(1.0);
        let far_a = matrix * Vec3::new(0.0, 2.0, 0.0).extend(1.0);
        let far_b = matrix * Vec3::new(1.0, 2.0, 0.0).extend(1.0);
        assert!(((near_b.x - near_a.x) - (far_b.x - far_a.x)).abs() < 1.0e-6);
    }
}
