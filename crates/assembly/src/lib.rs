//! Persistent rigid-part assembly data and renderer-independent hierarchy resolution.

mod exporter;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{EulerRot, Mat4, Quat, Vec3};
use openscad_render::{Mesh, RenderInstance, RenderScene};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use exporter::write_dae;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MeshSourceRef {
    ProjectSource { virtual_path: String },
}

impl MeshSourceRef {
    pub fn project_source(path: impl Into<String>) -> Self {
        Self::ProjectSource {
            virtual_path: path.into(),
        }
    }

    pub fn virtual_path(&self) -> &str {
        match self {
            Self::ProjectSource { virtual_path } => virtual_path,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translation: [f32; 3],
    pub rotation_degrees: [f32; 3],
    pub scale: [f32; 3],
    pub pivot: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation_degrees: [0.0; 3],
            scale: [1.0; 3],
            pivot: [0.0; 3],
        }
    }
}

impl Transform {
    pub fn matrix(self) -> Result<Mat4> {
        self.validate()?;
        let translation = Vec3::from_array(self.translation);
        let rotation = Vec3::from_array(self.rotation_degrees).map(f32::to_radians);
        let scale = Vec3::from_array(self.scale);
        let pivot = Vec3::from_array(self.pivot);
        Ok(Mat4::from_translation(translation)
            * Mat4::from_translation(pivot)
            * Mat4::from_quat(Quat::from_euler(
                EulerRot::XYZ,
                rotation.x,
                rotation.y,
                rotation.z,
            ))
            * Mat4::from_scale(scale)
            * Mat4::from_translation(-pivot))
    }

    pub fn validate(self) -> Result<()> {
        if self
            .translation
            .into_iter()
            .chain(self.rotation_degrees)
            .chain(self.scale)
            .chain(self.pivot)
            .any(|value| !value.is_finite())
        {
            return Err(AssemblyError::NonFiniteTransform);
        }
        if self
            .scale
            .into_iter()
            .any(|value| value.abs() <= f32::EPSILON)
        {
            return Err(AssemblyError::ZeroScale);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartInstance {
    pub id: String,
    pub name: String,
    pub source: MeshSourceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default = "default_visible")]
    pub visible: bool,
}

fn default_visible() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssemblyDocument {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartInstance>,
}

impl AssemblyDocument {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: identifier(&name),
            name,
            parts: Vec::new(),
        }
    }

    pub fn add_part(
        &mut self,
        source: MeshSourceRef,
        name: impl Into<String>,
    ) -> Result<&PartInstance> {
        let (name, id) = self.unique_part_identity(name.into());
        self.parts.push(PartInstance {
            id,
            name,
            source,
            parent: None,
            transform: Transform::default(),
            visible: true,
        });
        Ok(self.parts.last().expect("part was just pushed"))
    }

    fn unique_part_identity(&self, base_name: String) -> (String, String) {
        let mut name = base_name.clone();
        let mut id = identifier(&name);
        let mut suffix = 2;
        while self
            .parts
            .iter()
            .any(|part| part.name == name || part.id == id)
        {
            name = format!("{base_name}{suffix}");
            id = identifier(&name);
            suffix += 1;
        }
        (name, id)
    }

    pub fn part(&self, id_or_name: &str) -> Option<&PartInstance> {
        self.parts
            .iter()
            .find(|part| part.id == id_or_name || part.name == id_or_name)
    }

    pub fn part_mut(&mut self, id_or_name: &str) -> Option<&mut PartInstance> {
        self.parts
            .iter_mut()
            .find(|part| part.id == id_or_name || part.name == id_or_name)
    }

    pub fn remove_part(&mut self, id_or_name: &str) -> Result<PartInstance> {
        let index = self
            .parts
            .iter()
            .position(|part| part.id == id_or_name || part.name == id_or_name)
            .ok_or_else(|| AssemblyError::PartNotFound(id_or_name.to_string()))?;
        let removed = self.parts.remove(index);
        for part in &mut self.parts {
            if part.parent.as_deref() == Some(&removed.id) {
                part.parent = removed.parent.clone();
            }
        }
        Ok(removed)
    }

    pub fn set_parent(&mut self, child: &str, parent: Option<&str>) -> Result<()> {
        let child_id = self
            .part(child)
            .ok_or_else(|| AssemblyError::PartNotFound(child.to_string()))?
            .id
            .clone();
        let parent_id = parent
            .map(|value| {
                self.part(value)
                    .map(|part| part.id.clone())
                    .ok_or_else(|| AssemblyError::PartNotFound(value.to_string()))
            })
            .transpose()?;
        if parent_id.as_deref() == Some(&child_id) {
            return Err(AssemblyError::ParentCycle(child_id));
        }
        let previous_parent = self
            .part(&child_id)
            .expect("child resolved above")
            .parent
            .clone();
        self.part_mut(&child_id)
            .expect("child resolved above")
            .parent = parent_id;
        if let Err(error) = self.validate() {
            self.part_mut(&child_id).expect("child still exists").parent = previous_parent;
            return Err(error);
        }
        Ok(())
    }

    /// Return part indices in stable depth-first hierarchy order with their display depth.
    pub fn hierarchy_rows(&self) -> Vec<(usize, usize)> {
        fn append(
            document: &AssemblyDocument,
            parent: Option<&str>,
            depth: usize,
            visited: &mut HashSet<usize>,
            rows: &mut Vec<(usize, usize)>,
        ) {
            for (index, part) in document.parts.iter().enumerate() {
                if visited.contains(&index) || part.parent.as_deref() != parent {
                    continue;
                }
                visited.insert(index);
                rows.push((index, depth));
                append(document, Some(&part.id), depth + 1, visited, rows);
            }
        }

        let mut visited = HashSet::new();
        let mut rows = Vec::with_capacity(self.parts.len());
        append(self, None, 0, &mut visited, &mut rows);
        // Invalid persisted input should remain inspectable even before validation reports it.
        for index in 0..self.parts.len() {
            if visited.insert(index) {
                rows.push((index, 0));
            }
        }
        rows
    }

    pub fn validate(&self) -> Result<()> {
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for part in &self.parts {
            if part.id.is_empty() || !ids.insert(part.id.clone()) {
                return Err(AssemblyError::DuplicatePart(part.id.clone()));
            }
            if part.name.is_empty() || !names.insert(part.name.clone()) {
                return Err(AssemblyError::DuplicatePartName(part.name.clone()));
            }
            part.transform.validate()?;
        }
        for part in &self.parts {
            if let Some(parent) = &part.parent {
                if !ids.contains(parent) {
                    return Err(AssemblyError::MissingParent {
                        part: part.id.clone(),
                        parent: parent.clone(),
                    });
                }
            }
        }
        let indices = self
            .parts
            .iter()
            .enumerate()
            .map(|(index, part)| (part.id.as_str(), index))
            .collect::<HashMap<_, _>>();
        for part in &self.parts {
            let mut seen = HashSet::new();
            let mut current = Some(part.id.as_str());
            while let Some(id) = current {
                if !seen.insert(id) {
                    return Err(AssemblyError::ParentCycle(part.id.clone()));
                }
                current = self.parts[*indices.get(id).expect("validated ID")]
                    .parent
                    .as_deref();
            }
        }
        Ok(())
    }

    pub fn resolve(&self, meshes: &HashMap<MeshSourceRef, Arc<Mesh>>) -> Result<ResolvedAssembly> {
        self.validate()?;
        if self.parts.is_empty() {
            return Err(AssemblyError::EmptyAssembly);
        }
        let part_indices = self
            .parts
            .iter()
            .enumerate()
            .map(|(index, part)| (part.id.clone(), index))
            .collect::<HashMap<_, _>>();
        let mut geometry_indices = HashMap::new();
        let mut geometries = Vec::new();
        let mut nodes = Vec::with_capacity(self.parts.len());
        let mut world_cache = vec![None; self.parts.len()];
        for (index, part) in self.parts.iter().enumerate() {
            let mesh = meshes.get(&part.source).ok_or_else(|| {
                AssemblyError::MissingMesh(part.source.virtual_path().to_string())
            })?;
            let geometry_index =
                *geometry_indices
                    .entry(part.source.clone())
                    .or_insert_with(|| {
                        let index = geometries.len();
                        geometries.push(ResolvedGeometry {
                            source: part.source.clone(),
                            mesh: Arc::clone(mesh),
                        });
                        index
                    });
            let world_transform =
                resolve_world(index, &self.parts, &part_indices, &mut world_cache)?;
            nodes.push(ResolvedPart {
                id: part.id.clone(),
                name: part.name.clone(),
                parent_index: part
                    .parent
                    .as_ref()
                    .and_then(|parent| part_indices.get(parent).copied()),
                geometry_index,
                local_transform: part.transform.matrix()?,
                world_transform,
                visible: part.visible,
            });
        }
        Ok(ResolvedAssembly {
            id: self.id.clone(),
            name: self.name.clone(),
            geometries,
            nodes,
        })
    }
}

fn resolve_world(
    index: usize,
    parts: &[PartInstance],
    indices: &HashMap<String, usize>,
    cache: &mut [Option<Mat4>],
) -> Result<Mat4> {
    if let Some(world) = cache[index] {
        return Ok(world);
    }
    let part = &parts[index];
    let local = part.transform.matrix()?;
    let world = if let Some(parent) = &part.parent {
        resolve_world(indices[parent], parts, indices, cache)? * local
    } else {
        local
    };
    cache[index] = Some(world);
    Ok(world)
}

#[derive(Debug, Clone)]
pub struct ResolvedGeometry {
    pub source: MeshSourceRef,
    pub mesh: Arc<Mesh>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPart {
    pub id: String,
    pub name: String,
    pub parent_index: Option<usize>,
    pub geometry_index: usize,
    pub local_transform: Mat4,
    pub world_transform: Mat4,
    pub visible: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedAssembly {
    pub id: String,
    pub name: String,
    pub geometries: Vec<ResolvedGeometry>,
    pub nodes: Vec<ResolvedPart>,
}

impl ResolvedAssembly {
    pub fn render_scene(&self, selected: Option<&str>) -> Result<RenderScene> {
        let meshes = self
            .geometries
            .iter()
            .map(|geometry| Arc::clone(&geometry.mesh))
            .collect();
        let instances = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let mut instance = RenderInstance::new(node.geometry_index, node.world_transform);
                instance.visible = node.visible;
                instance.object_id = index as u32 + 1;
                if selected == Some(node.id.as_str()) {
                    instance.tint = Some([235, 185, 80, 255]);
                }
                instance
            })
            .collect();
        RenderScene::new(meshes, instances).map_err(AssemblyError::Render)
    }
}

fn identifier(name: &str) -> String {
    let value = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if value.is_empty() {
        "assembly".to_string()
    } else {
        value
    }
}

#[derive(Debug, Error)]
pub enum AssemblyError {
    #[error("assembly contains no parts")]
    EmptyAssembly,
    #[error("assembly part '{0}' was not found")]
    PartNotFound(String),
    #[error("assembly contains duplicate part ID '{0}'")]
    DuplicatePart(String),
    #[error("assembly contains an empty or duplicate part name '{0}'")]
    DuplicatePartName(String),
    #[error("part '{part}' references missing parent '{parent}'")]
    MissingParent { part: String, parent: String },
    #[error("part hierarchy contains a cycle involving '{0}'")]
    ParentCycle(String),
    #[error("assembly transform contains a non-finite value")]
    NonFiniteTransform,
    #[error("assembly scale components must be non-zero")]
    ZeroScale,
    #[error("mesh source '{0}' has not been compiled")]
    MissingMesh(String),
    #[error("assembly export failed: {0}")]
    Io(String),
    #[error(transparent)]
    Render(#[from] openscad_render::RenderError),
}

pub type Result<T> = std::result::Result<T, AssemblyError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh() -> Arc<Mesh> {
        Arc::new(Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]).unwrap())
    }

    #[test]
    fn resolves_parented_parts_and_reuses_geometry() {
        let source = MeshSourceRef::project_source("parts/cube.scad");
        let mut assembly = AssemblyDocument::new("Robot");
        let root = assembly
            .add_part(source.clone(), "body")
            .unwrap()
            .id
            .clone();
        let child = assembly.add_part(source.clone(), "arm").unwrap().id.clone();
        assembly.part_mut(&root).unwrap().transform.translation = [2.0, 0.0, 0.0];
        assembly.part_mut(&child).unwrap().transform.translation = [0.0, 3.0, 0.0];
        assembly.set_parent(&child, Some(&root)).unwrap();
        let resolved = assembly
            .resolve(&HashMap::from([(source, mesh())]))
            .unwrap();

        assert_eq!(resolved.geometries.len(), 1);
        assert_eq!(resolved.nodes.len(), 2);
        assert_eq!(
            resolved.nodes[1]
                .world_transform
                .transform_point3(Vec3::ZERO),
            Vec3::new(2.0, 3.0, 0.0)
        );
        assert_eq!(resolved.render_scene(None).unwrap().triangle_count(), 2);
    }

    #[test]
    fn rejects_cycles_zero_scale_and_missing_meshes() {
        let source = MeshSourceRef::project_source("part.scad");
        let mut assembly = AssemblyDocument::new("invalid");
        let a = assembly.add_part(source.clone(), "a").unwrap().id.clone();
        let b = assembly.add_part(source.clone(), "b").unwrap().id.clone();
        assembly.set_parent(&b, Some(&a)).unwrap();
        assert!(matches!(
            assembly.set_parent(&a, Some(&b)),
            Err(AssemblyError::ParentCycle(_))
        ));
        assembly.part_mut(&a).unwrap().transform.scale = [0.0, 1.0, 1.0];
        assert!(matches!(assembly.validate(), Err(AssemblyError::ZeroScale)));
        assembly.part_mut(&a).unwrap().transform.scale = [1.0; 3];
        assert!(matches!(
            assembly.resolve(&HashMap::new()),
            Err(AssemblyError::MissingMesh(_))
        ));
    }

    #[test]
    fn removing_a_parent_promotes_children() {
        let source = MeshSourceRef::project_source("part.scad");
        let mut assembly = AssemblyDocument::new("tree");
        let root = assembly
            .add_part(source.clone(), "root")
            .unwrap()
            .id
            .clone();
        let middle = assembly
            .add_part(source.clone(), "middle")
            .unwrap()
            .id
            .clone();
        let child = assembly.add_part(source, "child").unwrap().id.clone();
        assembly.set_parent(&middle, Some(&root)).unwrap();
        assembly.set_parent(&child, Some(&middle)).unwrap();

        assembly.remove_part(&middle).unwrap();

        assert_eq!(
            assembly.part(&child).unwrap().parent.as_deref(),
            Some(root.as_str())
        );
    }

    #[test]
    fn hierarchy_rows_keep_siblings_stable_and_indent_children() {
        let source = MeshSourceRef::project_source("part.scad");
        let mut assembly = AssemblyDocument::new("tree");
        let root = assembly
            .add_part(source.clone(), "root")
            .unwrap()
            .id
            .clone();
        let sibling = assembly
            .add_part(source.clone(), "sibling")
            .unwrap()
            .id
            .clone();
        let child = assembly.add_part(source, "child").unwrap().id.clone();
        assembly.set_parent(&child, Some(&root)).unwrap();

        let rows = assembly.hierarchy_rows();
        assert_eq!(
            rows.iter()
                .map(|(index, depth)| (assembly.parts[*index].id.as_str(), *depth))
                .collect::<Vec<_>>(),
            vec![
                (root.as_str(), 0),
                (child.as_str(), 1),
                (sibling.as_str(), 0)
            ]
        );
    }

    #[test]
    fn duplicate_part_names_receive_stable_display_and_id_suffixes() {
        let source = MeshSourceRef::project_source("part.scad");
        let mut assembly = AssemblyDocument::new("names");

        assembly.add_part(source.clone(), "arm").unwrap();
        assembly.add_part(source.clone(), "arm").unwrap();
        assembly.add_part(source, "arm").unwrap();

        assert_eq!(
            assembly
                .parts
                .iter()
                .map(|part| (part.name.as_str(), part.id.as_str()))
                .collect::<Vec<_>>(),
            vec![("arm", "arm"), ("arm2", "arm2"), ("arm3", "arm3")]
        );
    }

    #[test]
    fn rejects_persisted_duplicate_part_names_instead_of_migrating_them() {
        let source = MeshSourceRef::project_source("part.scad");
        let mut assembly = AssemblyDocument::new("names");
        assembly.add_part(source.clone(), "arm").unwrap();
        assembly.add_part(source, "leg").unwrap();
        assembly.parts[1].name = "arm".into();

        assert!(matches!(
            assembly.validate(),
            Err(AssemblyError::DuplicatePartName(name)) if name == "arm"
        ));
    }
}
