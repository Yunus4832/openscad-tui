use std::sync::Arc;

use crate::{Aabb, Mat4, Mesh, RenderError, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct RenderInstance {
    pub mesh_index: usize,
    pub transform: Mat4,
    pub visible: bool,
    pub tint: Option<[u8; 4]>,
    pub object_id: u32,
}

impl RenderInstance {
    pub fn new(mesh_index: usize, transform: Mat4) -> Self {
        Self {
            mesh_index,
            transform,
            visible: true,
            tint: None,
            object_id: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderScene {
    pub meshes: Vec<Arc<Mesh>>,
    pub instances: Vec<RenderInstance>,
    pub bounds: Aabb,
}

impl RenderScene {
    pub fn new(meshes: Vec<Arc<Mesh>>, instances: Vec<RenderInstance>) -> Result<Self> {
        let mut visible_bounds: Option<Aabb> = None;
        let mut all_bounds: Option<Aabb> = None;
        for (index, instance) in instances.iter().enumerate() {
            let mesh = meshes
                .get(instance.mesh_index)
                .ok_or(RenderError::InvalidMeshInstance {
                    index: instance.mesh_index,
                    mesh_count: meshes.len(),
                })?;
            if !instance.transform.is_finite() {
                return Err(RenderError::NonFiniteTransform { index });
            }
            let instance_bounds = mesh.bounds.transformed(instance.transform)?;
            all_bounds = Some(match all_bounds {
                Some(current) => current.union(instance_bounds),
                None => instance_bounds,
            });
            if instance.visible {
                visible_bounds = Some(match visible_bounds {
                    Some(current) => current.union(instance_bounds),
                    None => instance_bounds,
                });
            }
        }
        Ok(Self {
            meshes,
            instances,
            // Keep a stable camera when the user temporarily hides every instance.
            bounds: visible_bounds
                .or(all_bounds)
                .ok_or(RenderError::EmptyMesh)?,
        })
    }

    pub fn single(mesh: Mesh) -> Self {
        let bounds = mesh.bounds;
        Self {
            meshes: vec![Arc::new(mesh)],
            instances: vec![RenderInstance::new(0, Mat4::IDENTITY)],
            bounds,
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.instances
            .iter()
            .filter(|instance| instance.visible)
            .map(|instance| self.meshes[instance.mesh_index].triangle_count())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use crate::Vec3;

    use super::*;

    fn triangle() -> Arc<Mesh> {
        Arc::new(Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]).unwrap())
    }

    #[test]
    fn scene_counts_instances_and_combines_transformed_bounds() {
        let scene = RenderScene::new(
            vec![triangle()],
            vec![
                RenderInstance::new(0, Mat4::IDENTITY),
                RenderInstance::new(0, Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0))),
            ],
        )
        .unwrap();

        assert_eq!(scene.triangle_count(), 2);
        assert_eq!(scene.bounds.min, Vec3::ZERO);
        assert_eq!(scene.bounds.max, Vec3::new(6.0, 1.0, 0.0));
    }

    #[test]
    fn scene_rejects_invalid_instances_and_falls_back_to_hidden_bounds() {
        assert!(matches!(
            RenderScene::new(
                vec![triangle()],
                vec![RenderInstance::new(1, Mat4::IDENTITY)]
            ),
            Err(RenderError::InvalidMeshInstance { .. })
        ));
        let mut hidden = RenderInstance::new(0, Mat4::IDENTITY);
        hidden.visible = false;
        let scene = RenderScene::new(vec![triangle()], vec![hidden]).unwrap();
        assert_eq!(scene.bounds.min, Vec3::ZERO);
        assert_eq!(scene.triangle_count(), 0);
    }
}
