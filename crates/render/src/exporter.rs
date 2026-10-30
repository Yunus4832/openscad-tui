use std::fs;
use std::io::Write;
use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use xatlas_rs_v2::{ChartOptions, IndexData, MeshData, MeshDecl, PackOptions, Xatlas};

use crate::{Mat4, Mesh, RenderError, Result, Vec3};

/// One named node in an exported scene. Transforms use OpenSCAD's Z-up millimetres.
/// A node without a mesh still carries its transform and children (e.g. a hidden part).
pub struct ExportNode {
    pub name: String,
    pub parent: Option<usize>,
    pub mesh: Option<usize>,
    pub transform: Mat4,
}

/// Export a mesh with flat normals and an automatically packed UV atlas.
/// `.glb` embeds binary buffers; `.gltf` embeds them as a data URI, with no sidecars.
pub fn write_gltf(path: impl AsRef<Path>, mesh: &Mesh) -> Result<()> {
    write_gltf_scene(
        path,
        "OpenSCAD Model",
        &[mesh],
        &[ExportNode {
            name: "OpenSCAD Model".into(),
            parent: None,
            mesh: Some(0),
            transform: Mat4::IDENTITY,
        }],
    )
}

/// Export a static scene, preserving hierarchy and shared geometry.
/// A conversion root maps Z-up millimetres to glTF's Y-up metres.
pub fn write_gltf_scene(
    path: impl AsRef<Path>,
    name: &str,
    meshes: &[&Mesh],
    nodes: &[ExportNode],
) -> Result<()> {
    let path = path.as_ref();
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "glb" | "gltf") {
        return Err(export_error("expected a .glb or .gltf destination"));
    }
    validate_nodes(meshes.len(), nodes)?;
    let mut buffer = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut geometries = Vec::new();
    for (index, mesh) in meshes.iter().enumerate() {
        let mesh = unwrap_mesh(mesh)?;
        let positions = append_attribute(
            &mut buffer,
            &mut views,
            &mut accessors,
            &mesh.positions,
            "VEC3",
        );
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for point in &mesh.positions {
            for axis in 0..3 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        }
        accessors[positions]["min"] = json!(min);
        accessors[positions]["max"] = json!(max);
        let normals = append_attribute(
            &mut buffer,
            &mut views,
            &mut accessors,
            &mesh.normals,
            "VEC3",
        );
        let uv = append_attribute(&mut buffer, &mut views, &mut accessors, &mesh.uv, "VEC2");
        let start = buffer.len();
        for index in &mesh.indices {
            buffer.extend_from_slice(&index.to_le_bytes());
        }
        views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": buffer.len() - start, "target": 34963}));
        let indices = accessors.len();
        accessors.push(json!({"bufferView": views.len() - 1, "componentType": 5125, "count": mesh.indices.len(), "type": "SCALAR"}));
        geometries.push(json!({"name": format!("mesh-{index}"), "primitives": [{
            "attributes": {"POSITION": positions, "NORMAL": normals, "TEXCOORD_0": uv},
            "indices": indices, "mode": 4, "material": 0
        }]}));
    }
    let mut scene_nodes = Vec::new();
    let roots: Vec<_> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.parent.is_none())
        .map(|(i, _)| i + 1)
        .collect();
    // (x, y, z) -> (x, z, -y), with a uniform millimetre-to-metre scale.
    scene_nodes.push(json!({"name": name, "matrix": [0.001,0.0,0.0,0.0, 0.0,0.0,-0.001,0.0, 0.0,0.001,0.0,0.0, 0.0,0.0,0.0,1.0], "children": roots}));
    for (index, node) in nodes.iter().enumerate() {
        let mut value = json!({"name": node.name, "matrix": node.transform.to_cols_array()});
        if let Some(mesh) = node.mesh {
            value["mesh"] = json!(mesh);
        }
        let children: Vec<_> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent == Some(index))
            .map(|(i, _)| i + 1)
            .collect();
        if !children.is_empty() {
            value["children"] = json!(children);
        }
        scene_nodes.push(value);
    }
    let mut document = json!({
        "asset": {"version": "2.0", "generator": concat!("openscad-tui ", env!("CARGO_PKG_VERSION"))},
        "scene": 0, "scenes": [{"name": name, "nodes": [0]}],
        "nodes": scene_nodes, "meshes": geometries, "accessors": accessors,
        "bufferViews": views, "buffers": [{"byteLength": buffer.len()}],
        "materials": [{"name": "White", "pbrMetallicRoughness": {
            "baseColorFactor": [1.0, 1.0, 1.0, 1.0], "metallicFactor": 0.0, "roughnessFactor": 1.0
        }}]
    });
    let bytes = if extension == "gltf" {
        document["buffers"][0]["uri"] = json!(format!(
            "data:application/octet-stream;base64,{}",
            STANDARD.encode(&buffer)
        ));
        serde_json::to_vec_pretty(&document).map_err(|e| export_error(e.to_string()))?
    } else {
        encode_glb(&document, buffer)?
    };
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(io_error)?;
    // Finish generation before touching the destination, then replace it atomically.
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.flush().map_err(io_error)?;
    file.persist(path).map_err(|e| io_error(e.error))?;
    Ok(())
}

fn validate_nodes(mesh_count: usize, nodes: &[ExportNode]) -> Result<()> {
    if mesh_count == 0 || nodes.is_empty() {
        return Err(export_error("scene contains no geometry or nodes"));
    }
    for (index, node) in nodes.iter().enumerate() {
        if !node.transform.is_finite() {
            return Err(RenderError::NonFiniteTransform { index });
        }
        if let Some(mesh) = node.mesh {
            if mesh >= mesh_count {
                return Err(RenderError::InvalidMeshInstance {
                    index: mesh,
                    mesh_count,
                });
            }
        }
        let mut current = node.parent;
        for depth in 0..=nodes.len() {
            let Some(parent) = current else { break };
            if parent >= nodes.len() || parent == index || depth == nodes.len() {
                return Err(export_error("invalid or cyclic node hierarchy"));
            }
            current = nodes[parent].parent;
        }
    }
    Ok(())
}

struct UnwrappedMesh {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

fn unwrap_mesh(mesh: &Mesh) -> Result<UnwrappedMesh> {
    if mesh.triangles.is_empty() {
        return Err(export_error("mesh has no non-degenerate triangles"));
    }
    let count = mesh
        .triangles
        .len()
        .checked_mul(3)
        .ok_or_else(|| export_error("mesh is too large"))?;
    u32::try_from(count).map_err(|_| export_error("mesh exceeds 32-bit vertex limits"))?;
    let mut positions = Vec::with_capacity(count * 3);
    let mut normals = Vec::with_capacity(count * 3);
    // Split hard edges before unwrapping; xatlas can merge coplanar faces into charts.
    for triangle in &mesh.triangles {
        let mut points = [Vec3::ZERO; 3];
        for (corner, &index) in triangle.iter().enumerate() {
            points[corner] = *mesh
                .positions
                .get(index as usize)
                .ok_or_else(|| export_error("invalid triangle index"))?;
        }
        let normal = (points[1] - points[0])
            .cross(points[2] - points[0])
            .normalize();
        if !normal.is_finite() || points.iter().any(|p| !p.is_finite()) {
            return Err(export_error("mesh contains invalid or degenerate geometry"));
        }
        for point in points {
            positions.extend_from_slice(&point.to_array());
            normals.extend_from_slice(&normal.to_array());
        }
    }
    let indices: Vec<u32> = (0..count as u32).collect();
    let decl = MeshDecl {
        vertex_position_data: MeshData::Contiguous(&positions),
        vertex_normal_data: Some(MeshData::Contiguous(&normals)),
        index_data: Some(IndexData::U32(&indices)),
        ..Default::default()
    };
    let mut atlas = Xatlas::new();
    atlas
        .add_mesh(&decl)
        .map_err(|e| export_error(format!("UV input rejected: {e:?}")))?;
    atlas.generate(
        &ChartOptions {
            normal_seam_weight: 1001.0,
            ..Default::default()
        },
        &PackOptions {
            padding: 4,
            resolution: 1024,
            ..Default::default()
        },
    );
    if atlas.atlas_count() != 1 || atlas.width() == 0 || atlas.height() == 0 {
        return Err(export_error("UV generation did not produce a single atlas"));
    }
    let output = atlas.meshes();
    let output = output
        .first()
        .ok_or_else(|| export_error("UV generation returned no mesh"))?;
    if output.index_array.len() != count {
        return Err(export_error("UV generation changed the triangle count"));
    }
    let mut result = UnwrappedMesh {
        positions: Vec::new(),
        normals: Vec::new(),
        uv: Vec::new(),
        indices: output.index_array.to_vec(),
    };
    for vertex in &output.vertex_array {
        let offset = vertex.xref as usize * 3;
        if offset + 3 > positions.len() || vertex.atlas_index != 0 {
            return Err(export_error("UV generation returned an invalid vertex"));
        }
        let uv = [
            vertex.uv[0] / atlas.width() as f32,
            vertex.uv[1] / atlas.height() as f32,
        ];
        if uv
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(export_error("UV coordinate is outside the atlas"));
        }
        result
            .positions
            .push(positions[offset..offset + 3].try_into().unwrap());
        result
            .normals
            .push(normals[offset..offset + 3].try_into().unwrap());
        result.uv.push(uv);
    }
    for triangle in result.indices.chunks_exact(3) {
        let uv: Vec<_> = triangle
            .iter()
            .map(|&i| {
                result
                    .uv
                    .get(i as usize)
                    .copied()
                    .ok_or_else(|| export_error("invalid UV index"))
            })
            .collect::<Result<_>>()?;
        let area = (uv[1][0] - uv[0][0]) * (uv[2][1] - uv[0][1])
            - (uv[1][1] - uv[0][1]) * (uv[2][0] - uv[0][0]);
        if area == 0.0 {
            return Err(export_error("UV generation produced a degenerate triangle"));
        }
    }
    Ok(result)
}

fn append_attribute<const N: usize>(
    buffer: &mut Vec<u8>,
    views: &mut Vec<Value>,
    accessors: &mut Vec<Value>,
    values: &[[f32; N]],
    kind: &str,
) -> usize {
    let start = buffer.len();
    for value in values.iter().flatten() {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": buffer.len() - start, "target": 34962}));
    let index = accessors.len();
    accessors.push(json!({"bufferView": views.len() - 1, "componentType": 5126, "count": values.len(), "type": kind}));
    index
}

fn encode_glb(document: &Value, mut buffer: Vec<u8>) -> Result<Vec<u8>> {
    let mut json = serde_json::to_vec(document).map_err(|e| export_error(e.to_string()))?;
    json.resize(json.len().next_multiple_of(4), b' ');
    buffer.resize(buffer.len().next_multiple_of(4), 0);
    let total = 28usize
        .checked_add(json.len())
        .and_then(|n| n.checked_add(buffer.len()))
        .ok_or_else(|| export_error("GLB is too large"))?;
    let total = u32::try_from(total).map_err(|_| export_error("GLB exceeds the 4 GiB limit"))?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"glTF");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&total.to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"JSON");
    bytes.extend_from_slice(&json);
    bytes.extend_from_slice(&(buffer.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"BIN\0");
    bytes.extend_from_slice(&buffer);
    Ok(bytes)
}

fn export_error(message: impl Into<String>) -> RenderError {
    RenderError::GltfExport(message.into())
}
fn io_error(error: std::io::Error) -> RenderError {
    RenderError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Mesh {
        Mesh::new(
            vec![
                Vec3::ZERO,
                Vec3::X,
                Vec3::X + Vec3::Y,
                Vec3::Y,
                Vec3::Z,
                Vec3::X + Vec3::Z,
                Vec3::ONE,
                Vec3::Y + Vec3::Z,
            ],
            vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        )
        .unwrap()
    }

    fn triangles_overlap(a: [[f32; 2]; 3], b: [[f32; 2]; 3]) -> bool {
        for triangle in [a, b] {
            for edge in 0..3 {
                let p = triangle[edge];
                let q = triangle[(edge + 1) % 3];
                let axis = [p[1] - q[1], q[0] - p[0]];
                let project = |t: [[f32; 2]; 3]| {
                    t.map(|v| v[0] * axis[0] + v[1] * axis[1])
                        .into_iter()
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), v| {
                            (min.min(v), max.max(v))
                        })
                };
                let (amin, amax) = project(a);
                let (bmin, bmax) = project(b);
                if amax.min(bmax) - amin.max(bmin) <= 1.0e-7 {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn uv_atlas_has_no_overlaps_and_preserves_flat_normals() {
        let mesh = unwrap_mesh(&cube()).unwrap();
        assert_eq!(mesh.indices.len(), 36);
        assert!(mesh.positions.len() >= 24, "hard edges must be split");
        let triangles: Vec<_> = mesh
            .indices
            .chunks_exact(3)
            .map(|t| {
                [
                    mesh.uv[t[0] as usize],
                    mesh.uv[t[1] as usize],
                    mesh.uv[t[2] as usize],
                ]
            })
            .collect();
        for (i, a) in triangles.iter().enumerate() {
            for b in &triangles[i + 1..] {
                assert!(!triangles_overlap(*a, *b), "UV interiors overlap");
            }
        }
        for t in mesh.indices.chunks_exact(3) {
            let p = t
                .iter()
                .map(|&i| Vec3::from_array(mesh.positions[i as usize]))
                .collect::<Vec<_>>();
            let expected = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
            for &i in t {
                assert!(expected.distance(Vec3::from_array(mesh.normals[i as usize])) < 1.0e-6);
            }
        }
    }

    #[test]
    fn curved_mesh_generates_finite_non_overlapping_uvs() {
        let segments = 24u32;
        let mut positions = Vec::new();
        for z in [0.0, 2.0] {
            for i in 0..segments {
                let theta = i as f32 * std::f32::consts::TAU / segments as f32;
                positions.push(Vec3::new(theta.cos(), theta.sin(), z));
            }
        }
        let mut triangles = Vec::new();
        for i in 0..segments {
            let next = (i + 1) % segments;
            triangles.extend([
                [i, next, next + segments],
                [i, next + segments, i + segments],
            ]);
        }
        let mesh = unwrap_mesh(&Mesh::new(positions, triangles).unwrap()).unwrap();
        let triangles: Vec<_> = mesh
            .indices
            .chunks_exact(3)
            .map(|t| {
                [
                    mesh.uv[t[0] as usize],
                    mesh.uv[t[1] as usize],
                    mesh.uv[t[2] as usize],
                ]
            })
            .collect();
        for (i, a) in triangles.iter().enumerate() {
            for b in &triangles[i + 1..] {
                assert!(!triangles_overlap(*a, *b));
            }
        }
    }

    #[test]
    fn glb_and_gltf_round_trip_through_independent_parser() {
        let directory = tempfile::tempdir().unwrap();
        for extension in ["glb", "gltf", "GLB", "GLTF"] {
            let path = directory.path().join(format!("nested/cube.{extension}"));
            write_gltf(&path, &cube()).unwrap();
            let bytes = fs::read(&path).unwrap();
            let parsed = gltf::Gltf::from_slice(&bytes).unwrap();
            let binary = match &parsed.blob {
                Some(blob) => blob.clone(),
                None => {
                    let gltf::buffer::Source::Uri(uri) = parsed.buffers().next().unwrap().source()
                    else {
                        panic!()
                    };
                    STANDARD.decode(uri.split_once(',').unwrap().1).unwrap()
                }
            };
            assert_eq!(parsed.meshes().count(), 1);
            let primitive = parsed.meshes().next().unwrap().primitives().next().unwrap();
            let reader = primitive.reader(|_| Some(binary.as_slice()));
            let positions = reader.read_positions().unwrap().collect::<Vec<_>>();
            let normals = reader.read_normals().unwrap().collect::<Vec<_>>();
            let uv = reader
                .read_tex_coords(0)
                .unwrap()
                .into_f32()
                .collect::<Vec<_>>();
            assert_eq!(positions.len(), normals.len());
            assert_eq!(positions.len(), uv.len());
            assert_eq!(reader.read_indices().unwrap().into_u32().count(), 36);
            let root = parsed.scenes().next().unwrap().nodes().next().unwrap();
            let matrix = Mat4::from_cols_array_2d(&root.transform().matrix());
            assert!(
                matrix
                    .transform_point3(Vec3::new(1000.0, 2000.0, 3000.0))
                    .distance(Vec3::new(1.0, 3.0, -2.0))
                    < 1.0e-6
            );
            if extension.eq_ignore_ascii_case("glb") {
                assert_eq!(
                    u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
                    bytes.len()
                );
                assert_eq!(bytes.len() % 4, 0);
            }
        }
    }

    #[test]
    fn invalid_exports_preserve_existing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("existing.glb");
        fs::write(&path, b"previous model").unwrap();
        let empty = Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0], vec![[0, 1, 2]]).unwrap();
        assert!(write_gltf(&path, &empty).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"previous model");
        assert!(write_gltf(directory.path().join("removed.dae"), &cube()).is_err());
        assert!(!directory.path().join("removed.dae").exists());
        for (parent, mesh, transform) in [
            (Some(0), Some(0), Mat4::IDENTITY),
            (Some(99), Some(0), Mat4::IDENTITY),
            (None, Some(3), Mat4::IDENTITY),
            (None, Some(0), Mat4::from_cols_array(&[f32::NAN; 16])),
        ] {
            let node = ExportNode {
                name: "invalid".into(),
                parent,
                mesh,
                transform,
            };
            assert!(write_gltf_scene(&path, "invalid", &[&cube()], &[node]).is_err());
        }
        assert_eq!(fs::read(&path).unwrap(), b"previous model");
    }
}
