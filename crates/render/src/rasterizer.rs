use crate::{
    Aabb, Camera, Framebuffer, Mesh, PixelSize, RenderOptions, RenderScene, RgbaFrame, Vec2, Vec3,
    Vec4,
};

const AREA_EPSILON: f32 = 1.0e-6;
const AXIS_DEPTH_BIAS: f32 = 1.0e-4;
const AXIS_LENGTH_MARGIN: f32 = 1.1;
const AXIS_COLORS: [[u8; 4]; 3] = [[255, 96, 96, 255], [96, 255, 128, 255], [96, 160, 255, 255]];
const NEGATIVE_AXIS_COLORS: [[u8; 4]; 3] =
    [[128, 52, 52, 255], [52, 128, 68, 255], [52, 84, 128, 255]];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSettings {
    pub background: [u8; 4],
    pub base_color: [u8; 4],
    pub ambient: f32,
    pub diffuse: f32,
    /// Unit direction from the surface toward the light in camera/view space.
    /// The light therefore follows camera orbit and roll.
    pub light_direction: Vec3,
    pub backface_culling: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            background: [20, 24, 32, 255],
            base_color: [135, 180, 220, 255],
            ambient: 0.28,
            diffuse: 0.72,
            light_direction: Vec3::new(-1.0, 1.0, 1.0).normalize(),
            backface_culling: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CpuRenderer {
    settings: RenderSettings,
}

impl CpuRenderer {
    pub fn new(settings: RenderSettings) -> Self {
        Self { settings }
    }

    pub fn settings(&self) -> &RenderSettings {
        &self.settings
    }

    pub fn settings_mut(&mut self) -> &mut RenderSettings {
        &mut self.settings
    }

    pub fn render(&self, mesh: &Mesh, camera: &Camera, size: PixelSize) -> RgbaFrame {
        self.render_with_options(mesh, camera, size, RenderOptions::default())
    }

    pub fn render_with_options(
        &self,
        mesh: &Mesh,
        camera: &Camera,
        size: PixelSize,
        options: RenderOptions,
    ) -> RgbaFrame {
        self.render_scene_with_options(&RenderScene::single(mesh.clone()), camera, size, options)
    }

    pub fn render_scene_with_options(
        &self,
        scene: &RenderScene,
        camera: &Camera,
        size: PixelSize,
        options: RenderOptions,
    ) -> RgbaFrame {
        let mut framebuffer = Framebuffer::new(size, self.settings.background);
        let view_projection = camera.view_projection(size.aspect_ratio());
        let light_direction = world_light_direction(self.settings, camera);

        for instance in scene.instances.iter().filter(|instance| instance.visible) {
            let mesh = &scene.meshes[instance.mesh_index];
            let model = instance.transform;
            let model_view_projection = view_projection * model;
            let base_color = instance.tint.unwrap_or(self.settings.base_color);
            for triangle in &mesh.triangles {
                let local = triangle.map(|index| mesh.positions[index as usize]);
                let world = local.map(|position| model.transform_point3(position));
                let normal = (world[1] - world[0]).cross(world[2] - world[0]);
                if normal.length_squared() <= f32::EPSILON {
                    continue;
                }
                let clip_triangle =
                    local.map(|position| model_view_projection * position.extend(1.0));
                let polygon = clip_polygon(clip_triangle.to_vec());
                if polygon.len() < 3 {
                    continue;
                }
                let color = shade_with_color(
                    self.settings,
                    normal.normalize(),
                    light_direction,
                    base_color,
                );
                for offset in 1..polygon.len() - 1 {
                    self.rasterize_triangle(
                        &mut framebuffer,
                        [polygon[0], polygon[offset], polygon[offset + 1]],
                        color,
                    );
                }
            }
        }

        if options.axes {
            self.rasterize_axes(&mut framebuffer, scene.bounds, view_projection);
        }

        framebuffer.into_color()
    }

    fn rasterize_axes(&self, framebuffer: &mut Framebuffer, bounds: Aabb, transform: glam::Mat4) {
        let length = bounds
            .min
            .abs()
            .max(bounds.max.abs())
            .max_element()
            .max(bounds.radius())
            .max(1.0)
            * AXIS_LENGTH_MARGIN;
        for (index, direction) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
            self.rasterize_line(
                framebuffer,
                [
                    transform * Vec3::ZERO.extend(1.0),
                    transform * (direction * length).extend(1.0),
                ],
                AXIS_COLORS[index],
            );
            self.rasterize_line(
                framebuffer,
                [
                    transform * (-direction * length).extend(1.0),
                    transform * Vec3::ZERO.extend(1.0),
                ],
                NEGATIVE_AXIS_COLORS[index],
            );
        }
    }

    fn rasterize_line(&self, framebuffer: &mut Framebuffer, clip: [Vec4; 2], color: [u8; 4]) {
        let Some(clip) = clip_line(clip) else {
            return;
        };
        if clip.iter().any(|vertex| vertex.w.abs() <= f32::EPSILON) {
            return;
        }
        let size = framebuffer.size();
        let screen = clip.map(|vertex| clip_to_screen(vertex, size));
        let delta = screen[1].position - screen[0].position;
        let steps = delta.x.abs().max(delta.y.abs()).ceil().max(1.0) as u32;
        for step in 0..=steps {
            let factor = step as f32 / steps as f32;
            let position = screen[0].position.lerp(screen[1].position, factor);
            let depth = (screen[0].depth + (screen[1].depth - screen[0].depth) * factor
                - AXIS_DEPTH_BIAS)
                .clamp(0.0, 1.0);
            let x = position.x.round() as i32;
            let y = position.y.round() as i32;
            if x >= 0 && y >= 0 {
                framebuffer.write_pixel(x as u32, y as u32, depth, color);
            }
        }
    }

    fn rasterize_triangle(&self, framebuffer: &mut Framebuffer, clip: [Vec4; 3], color: [u8; 4]) {
        if clip.iter().any(|vertex| vertex.w.abs() <= f32::EPSILON) {
            return;
        }
        let size = framebuffer.size();
        let screen = clip.map(|vertex| clip_to_screen(vertex, size));
        let area = edge(screen[0].position, screen[1].position, screen[2].position);
        if area.abs() <= AREA_EPSILON || (self.settings.backface_culling && area >= 0.0) {
            return;
        }

        let min_x = screen
            .iter()
            .map(|vertex| vertex.position.x)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as u32;
        let max_x = screen
            .iter()
            .map(|vertex| vertex.position.x)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(size.width.saturating_sub(1) as f32) as u32;
        let min_y = screen
            .iter()
            .map(|vertex| vertex.position.y)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as u32;
        let max_y = screen
            .iter()
            .map(|vertex| vertex.position.y)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(size.height.saturating_sub(1) as f32) as u32;

        let edges = [
            (screen[1].position, screen[2].position),
            (screen[2].position, screen[0].position),
            (screen[0].position, screen[1].position),
        ];
        let step_x = edges.map(|(a, b)| -(b.y - a.y));
        let step_y = edges.map(|(a, b)| b.x - a.x);
        let first_sample = Vec2::new(min_x as f32 + 0.5, min_y as f32 + 0.5);
        let mut row_edges = edges.map(|(a, b)| edge(a, b, first_sample));
        let inverse_area = area.recip();

        for y in min_y..=max_y {
            let mut edge_values = row_edges;
            for x in min_x..=max_x {
                let weights = edge_values.map(|value| value * inverse_area);
                if weights[0] < -AREA_EPSILON
                    || weights[1] < -AREA_EPSILON
                    || weights[2] < -AREA_EPSILON
                {
                    for index in 0..3 {
                        edge_values[index] += step_x[index];
                    }
                    continue;
                }
                let depth = screen[0].depth * weights[0]
                    + screen[1].depth * weights[1]
                    + screen[2].depth * weights[2];
                if (0.0..=1.0).contains(&depth) {
                    framebuffer.write_pixel(x, y, depth, color);
                }
                for index in 0..3 {
                    edge_values[index] += step_x[index];
                }
            }
            for index in 0..3 {
                row_edges[index] += step_y[index];
            }
        }
    }
}

impl Default for CpuRenderer {
    fn default() -> Self {
        Self::new(RenderSettings::default())
    }
}

impl crate::FrameRenderer for CpuRenderer {
    fn render_frame(
        &self,
        scene: &RenderScene,
        camera: &Camera,
        size: PixelSize,
        options: RenderOptions,
    ) -> crate::Result<RgbaFrame> {
        Ok(self.render_scene_with_options(scene, camera, size, options))
    }
}

#[derive(Debug, Clone, Copy)]
struct ScreenVertex {
    position: Vec2,
    depth: f32,
}

fn clip_to_screen(vertex: Vec4, size: PixelSize) -> ScreenVertex {
    let ndc = vertex.truncate() / vertex.w;
    ScreenVertex {
        position: Vec2::new(
            (ndc.x * 0.5 + 0.5) * (size.width.saturating_sub(1)) as f32,
            (1.0 - (ndc.y * 0.5 + 0.5)) * (size.height.saturating_sub(1)) as f32,
        ),
        depth: ndc.z * 0.5 + 0.5,
    }
}

fn edge(a: Vec2, b: Vec2, point: Vec2) -> f32 {
    (b.x - a.x) * (point.y - a.y) - (b.y - a.y) * (point.x - a.x)
}

fn world_light_direction(settings: RenderSettings, camera: &Camera) -> Vec3 {
    camera
        .view_matrix()
        .inverse()
        .transform_vector3(settings.light_direction)
        .normalize_or_zero()
}

#[cfg(test)]
fn shade(settings: RenderSettings, normal: Vec3, light_direction: Vec3) -> [u8; 4] {
    shade_with_color(settings, normal, light_direction, settings.base_color)
}

fn shade_with_color(
    settings: RenderSettings,
    normal: Vec3,
    light_direction: Vec3,
    base_color: [u8; 4],
) -> [u8; 4] {
    // Match OpenSCAD's pair of opposed directional lights. The contribution
    // max(N·L, 0) + max(N·-L, 0) simplifies to abs(N·L), giving every
    // orientation a camera-relative fill light while preserving hard CAD edges.
    let intensity =
        (settings.ambient + settings.diffuse * normal.dot(light_direction).abs()).clamp(0.0, 1.0);
    [
        (base_color[0] as f32 * intensity).round() as u8,
        (base_color[1] as f32 * intensity).round() as u8,
        (base_color[2] as f32 * intensity).round() as u8,
        base_color[3],
    ]
}

fn clip_polygon(mut polygon: Vec<Vec4>) -> Vec<Vec4> {
    let planes: [fn(Vec4) -> f32; 6] = [
        |vertex| vertex.x + vertex.w,
        |vertex| vertex.w - vertex.x,
        |vertex| vertex.y + vertex.w,
        |vertex| vertex.w - vertex.y,
        |vertex| vertex.z + vertex.w,
        |vertex| vertex.w - vertex.z,
    ];
    for distance in planes {
        if polygon.is_empty() {
            break;
        }
        let input = std::mem::take(&mut polygon);
        let mut previous = *input.last().expect("non-empty polygon");
        let mut previous_distance = distance(previous);
        for current in input {
            let current_distance = distance(current);
            let previous_inside = previous_distance >= 0.0;
            let current_inside = current_distance >= 0.0;
            if previous_inside != current_inside {
                let factor = previous_distance / (previous_distance - current_distance);
                polygon.push(previous.lerp(current, factor));
            }
            if current_inside {
                polygon.push(current);
            }
            previous = current;
            previous_distance = current_distance;
        }
    }
    polygon
}

fn clip_line(mut line: [Vec4; 2]) -> Option<[Vec4; 2]> {
    let planes: [fn(Vec4) -> f32; 6] = [
        |vertex| vertex.x + vertex.w,
        |vertex| vertex.w - vertex.x,
        |vertex| vertex.y + vertex.w,
        |vertex| vertex.w - vertex.y,
        |vertex| vertex.z + vertex.w,
        |vertex| vertex.w - vertex.z,
    ];
    for distance in planes {
        let distances = line.map(distance);
        if distances[0] < 0.0 && distances[1] < 0.0 {
            return None;
        }
        if (distances[0] < 0.0) != (distances[1] < 0.0) {
            let factor = distances[0] / (distances[0] - distances[1]);
            let intersection = line[0].lerp(line[1], factor);
            if distances[0] < 0.0 {
                line[0] = intersection;
            } else {
                line[1] = intersection;
            }
        }
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{Aabb, Mat4, Projection, RenderInstance, RenderScene, StandardView};

    fn top_camera(mesh: &Mesh) -> Camera {
        let mut camera = Camera {
            projection: Projection::Orthographic { vertical_size: 3.0 },
            ..Camera::default()
        };
        camera.set_standard_view(StandardView::Top, mesh.bounds, 1.0);
        camera
    }

    #[test]
    fn rasterizes_a_visible_triangle() {
        let mesh = Mesh::new(
            vec![
                Vec3::new(-1.0, -1.0, 0.0),
                Vec3::new(1.0, -1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let settings = RenderSettings {
            backface_culling: false,
            ..RenderSettings::default()
        };
        let frame = CpuRenderer::new(settings).render(
            &mesh,
            &top_camera(&mesh),
            PixelSize::new(64, 64).unwrap(),
        );
        let changed = frame
            .pixels()
            .chunks_exact(4)
            .filter(|pixel| *pixel != settings.background)
            .count();
        assert!(changed > 500);
        assert!(changed < 2500);
    }

    #[test]
    fn rasterizes_multiple_transformed_instances_without_flattening_meshes() {
        let mesh = Arc::new(
            Mesh::new(
                vec![
                    Vec3::new(-0.4, -0.8, 0.0),
                    Vec3::new(0.4, -0.8, 0.0),
                    Vec3::new(0.0, 0.8, 0.0),
                ],
                vec![[0, 1, 2]],
            )
            .unwrap(),
        );
        let scene = RenderScene::new(
            vec![mesh],
            vec![
                RenderInstance::new(0, Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0))),
                RenderInstance::new(0, Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0))),
            ],
        )
        .unwrap();
        let size = PixelSize::new(96, 64).unwrap();
        let mut camera = Camera {
            projection: Projection::Orthographic { vertical_size: 3.0 },
            ..Camera::default()
        };
        camera.set_standard_view(StandardView::Top, scene.bounds, size.aspect_ratio());
        let renderer = CpuRenderer::new(RenderSettings {
            backface_culling: false,
            ..RenderSettings::default()
        });

        let frame = renderer.render_scene_with_options(
            &scene,
            &camera,
            size,
            RenderOptions { axes: false },
        );
        let background = renderer.settings.background;
        let changed_in_half = |start, end| {
            (0..size.height).any(|y| {
                (start..end).any(|x| {
                    let offset = (y as usize * size.width as usize + x as usize) * 4;
                    frame.pixels()[offset..offset + 4] != background
                })
            })
        };
        assert!(changed_in_half(0, size.width / 2));
        assert!(changed_in_half(size.width / 2, size.width));
        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.triangle_count(), 2);
    }

    #[test]
    fn world_axes_are_optional_and_use_xyz_colors() {
        let mesh = Mesh::new(
            vec![
                Vec3::new(-1.0, -1.0, 0.0),
                Vec3::new(1.0, -1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let renderer = CpuRenderer::default();
        let size = PixelSize::new(96, 96).unwrap();
        let camera = top_camera(&mesh);

        let without_axes =
            renderer.render_with_options(&mesh, &camera, size, RenderOptions { axes: false });
        let with_axes =
            renderer.render_with_options(&mesh, &camera, size, RenderOptions { axes: true });

        assert_ne!(with_axes, without_axes);
        assert!(with_axes
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel == AXIS_COLORS[0]));
        assert!(with_axes
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel == AXIS_COLORS[1]));

        let axis_mesh = Mesh::new(vec![Vec3::splat(-1.0), Vec3::splat(1.0)], Vec::new()).unwrap();
        let mut perspective = Camera::default();
        perspective.set_standard_view(
            StandardView::Isometric,
            axis_mesh.bounds,
            size.aspect_ratio(),
        );
        let perspective_frame = renderer.render_with_options(
            &axis_mesh,
            &perspective,
            size,
            RenderOptions { axes: true },
        );
        for color in AXIS_COLORS {
            assert!(
                perspective_frame
                    .pixels()
                    .chunks_exact(4)
                    .any(|pixel| pixel == color),
                "missing axis color {color:?}"
            );
        }
    }

    #[test]
    fn axis_lines_are_one_pixel_wide() {
        let renderer = CpuRenderer::default();
        let size = PixelSize::new(32, 16).unwrap();
        let mut framebuffer = Framebuffer::new(size, renderer.settings.background);

        renderer.rasterize_line(
            &mut framebuffer,
            [
                Vec4::new(-0.8, 0.0, 0.0, 1.0),
                Vec4::new(0.8, 0.0, 0.0, 1.0),
            ],
            AXIS_COLORS[0],
        );

        let frame = framebuffer.into_color();
        let colored_per_column = (0..size.width)
            .map(|x| {
                (0..size.height)
                    .filter(|&y| {
                        let offset = (y as usize * size.width as usize + x as usize) * 4;
                        frame.pixels()[offset..offset + 4] == AXIS_COLORS[0]
                    })
                    .count()
            })
            .collect::<Vec<_>>();
        assert!(colored_per_column.contains(&1));
        assert!(colored_per_column.iter().all(|&count| count <= 1));
    }

    #[test]
    fn model_depth_occludes_world_axes() {
        let mesh = Mesh::new(
            vec![
                Vec3::new(-2.0, -2.0, 0.5),
                Vec3::new(2.0, -2.0, 0.5),
                Vec3::new(2.0, 2.0, 0.5),
                Vec3::new(-2.0, 2.0, 0.5),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
        .unwrap();
        let camera = top_camera(&mesh);
        let size = PixelSize::new(96, 96).unwrap();
        let renderer = CpuRenderer::new(RenderSettings {
            backface_culling: false,
            ..RenderSettings::default()
        });
        let frame =
            renderer.render_with_options(&mesh, &camera, size, RenderOptions { axes: true });
        let projected_axis_point = clip_to_screen(
            camera.view_projection(size.aspect_ratio()) * Vec3::X.extend(1.0),
            size,
        );
        let x = projected_axis_point.position.x.round() as u32;
        let y = projected_axis_point.position.y.round() as u32;
        let offset = (y as usize * size.width as usize + x as usize) * 4;

        assert!(!AXIS_COLORS.contains(&frame.pixels()[offset..offset + 4].try_into().unwrap()));
        assert!(
            !NEGATIVE_AXIS_COLORS.contains(&frame.pixels()[offset..offset + 4].try_into().unwrap())
        );
    }

    #[test]
    fn clips_a_triangle_crossing_the_view_boundary() {
        let mesh = Mesh::new(
            vec![
                Vec3::new(-10.0, -1.0, 0.0),
                Vec3::new(10.0, -1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let mut camera = top_camera(&mesh);
        camera.projection = Projection::Orthographic { vertical_size: 2.5 };
        let settings = RenderSettings {
            backface_culling: false,
            ..RenderSettings::default()
        };
        let frame =
            CpuRenderer::new(settings).render(&mesh, &camera, PixelSize::new(32, 32).unwrap());
        assert!(frame
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel != settings.background));
    }

    #[test]
    fn backface_culling_removes_reversed_triangle() {
        let positions = vec![
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ];
        let front = Mesh::new(positions.clone(), vec![[0, 1, 2]]).unwrap();
        let back = Mesh::new(positions, vec![[2, 1, 0]]).unwrap();
        let renderer = CpuRenderer::default();
        let size = PixelSize::new(32, 32).unwrap();
        let front_frame = renderer.render_with_options(
            &front,
            &top_camera(&front),
            size,
            RenderOptions { axes: false },
        );
        let back_frame = renderer.render_with_options(
            &back,
            &top_camera(&back),
            size,
            RenderOptions { axes: false },
        );
        let background = renderer.settings.background;
        let front_count = front_frame
            .pixels()
            .chunks_exact(4)
            .filter(|pixel| *pixel != background)
            .count();
        let back_count = back_frame
            .pixels()
            .chunks_exact(4)
            .filter(|pixel| *pixel != background)
            .count();
        assert_ne!(front_count, back_count);
        assert_eq!(front_count.min(back_count), 0);
    }

    #[test]
    fn opposed_lights_illuminate_both_face_directions_equally() {
        let settings = RenderSettings {
            ambient: 0.2,
            diffuse: 0.8,
            light_direction: Vec3::Z,
            ..RenderSettings::default()
        };
        assert_eq!(
            shade(settings, Vec3::Z, Vec3::Z),
            shade(settings, -Vec3::Z, Vec3::Z)
        );
        assert!(shade(settings, Vec3::Z, Vec3::Z)[0] > shade(settings, Vec3::X, Vec3::Z)[0]);
    }

    #[test]
    fn light_direction_remains_fixed_in_camera_space() {
        let settings = RenderSettings::default();
        let mesh = Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]).unwrap();
        let mut camera = top_camera(&mesh);

        for (yaw, pitch) in [(0.0, 0.0), (0.7, -0.2), (-1.1, 0.4)] {
            camera.orbit(yaw, pitch);
            let world_direction = world_light_direction(settings, &camera);
            let view_direction = camera
                .view_matrix()
                .transform_vector3(world_direction)
                .normalize();
            assert!(view_direction.distance(settings.light_direction) < 1.0e-5);
        }
    }

    #[test]
    fn fit_bounds_fixture_is_non_degenerate() {
        let bounds = Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        assert!(bounds.radius() > 0.0);
    }
}
