use std::path::Path;

use openscad_render::{write_gltf_scene, ExportNode};

use crate::{ResolvedAssembly, Result};

/// Export a static glTF/GLB scene with shared meshes and automatic UV atlases.
/// Hidden parts retain their hierarchy and transforms but have no mesh reference.
pub fn write_gltf(path: impl AsRef<Path>, assembly: &ResolvedAssembly) -> Result<()> {
    let meshes: Vec<_> = assembly
        .geometries
        .iter()
        .map(|geometry| geometry.mesh.as_ref())
        .collect();
    let nodes: Vec<_> = assembly
        .nodes
        .iter()
        .map(|node| ExportNode {
            name: node.name.clone(),
            parent: node.parent_index,
            mesh: node.visible.then_some(node.geometry_index),
            transform: node.local_transform,
        })
        .collect();
    write_gltf_scene(path, &assembly.name, &meshes, &nodes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssemblyDocument, MeshSourceRef};
    use openscad_render::{Mat4, Mesh, Vec3};
    use std::{collections::HashMap, sync::Arc};

    #[test]
    fn preserves_hierarchy_instances_hidden_parent_and_local_matrices() {
        let source = MeshSourceRef::project_source("part.scad");
        let mesh =
            Arc::new(Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]).unwrap());
        let mut document = AssemblyDocument::new("Robot");
        let root = document
            .add_part(source.clone(), "Body")
            .unwrap()
            .id
            .clone();
        let child = document.add_part(source.clone(), "Arm").unwrap().id.clone();
        let sibling = document.add_part(source.clone(), "Arm").unwrap().id.clone();
        document.set_parent(&child, Some(&root)).unwrap();
        let part = document.part_mut(&root).unwrap();
        part.visible = false;
        part.transform.translation = [3.0, 5.0, 7.0];
        part.transform.rotation_degrees = [20.0, 30.0, 45.0];
        part.transform.scale = [-2.0, 3.0, 4.0];
        part.transform.pivot = [1.0, 2.0, 0.0];
        document.part_mut(&child).unwrap().transform.translation = [0.0, 0.0, 1000.0];
        let assembly = document.resolve(&HashMap::from([(source, mesh)])).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("robot.gltf");
        write_gltf(&path, &assembly).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(json["meshes"].as_array().unwrap().len(), 1);
        assert_eq!(json["nodes"][0]["children"], serde_json::json!([1, 3]));
        assert_eq!(json["nodes"][1]["children"], serde_json::json!([2]));
        assert!(json["nodes"][1].get("mesh").is_none());
        assert_eq!(json["nodes"][2]["mesh"], 0);
        assert_eq!(json["nodes"][3]["mesh"], 0);
        assert_eq!(json["nodes"][2]["name"], "Arm");
        assert_eq!(
            json["nodes"][3]["name"],
            document.part(&sibling).unwrap().name
        );
        let matrix = |index: usize| {
            Mat4::from_cols_array(&std::array::from_fn(|i| {
                json["nodes"][index]["matrix"][i].as_f64().unwrap() as f32
            }))
        };
        for point in [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z] {
            assert!(
                (matrix(1) * matrix(2))
                    .transform_point3(point)
                    .distance(assembly.nodes[1].world_transform.transform_point3(point))
                    < 1.0e-5
            );
        }
    }
}
