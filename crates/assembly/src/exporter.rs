use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::{AssemblyError, ResolvedAssembly, Result};

/// Write a static COLLADA 1.4.1 scene whose geometries are shared by part instances.
///
/// The export intentionally contains no materials, UVs, animation, skinning, cameras, or lights.
pub fn write_dae(path: impl AsRef<Path>, assembly: &ResolvedAssembly) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let file = File::create(path).map_err(io_error)?;
    write_dae_to(BufWriter::new(file), assembly)
}

fn write_dae_to(mut output: impl Write, assembly: &ResolvedAssembly) -> Result<()> {
    writeln!(output, r#"<?xml version="1.0" encoding="utf-8"?>"#).map_err(io_error)?;
    writeln!(
        output,
        r#"<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">"#
    )
    .map_err(io_error)?;
    writeln!(output, "  <asset>").map_err(io_error)?;
    writeln!(output, "    <created>1970-01-01T00:00:00Z</created>").map_err(io_error)?;
    writeln!(output, "    <modified>1970-01-01T00:00:00Z</modified>").map_err(io_error)?;
    writeln!(output, r#"    <unit name="millimeter" meter="0.001"/>"#).map_err(io_error)?;
    writeln!(output, "    <up_axis>Z_UP</up_axis>").map_err(io_error)?;
    writeln!(output, "  </asset>").map_err(io_error)?;
    writeln!(output, "  <library_geometries>").map_err(io_error)?;
    for (index, geometry) in assembly.geometries.iter().enumerate() {
        let id = format!("geometry-{index}");
        let name = assembly
            .nodes
            .iter()
            .find(|node| node.geometry_index == index)
            .map(|node| format!("{}-mesh", collada_name(&node.id, "part")))
            .unwrap_or_else(|| id.clone());
        let mesh = &geometry.mesh;
        writeln!(
            output,
            "    <geometry id=\"{id}\" name=\"{}\"><mesh>",
            xml_escape(&name)
        )
        .map_err(io_error)?;
        writeln!(
            output,
            "      <source id=\"{id}-positions\"><float_array id=\"{id}-positions-array\" count=\"{}\">",
            mesh.positions.len() * 3
        )
        .map_err(io_error)?;
        write!(output, "        ").map_err(io_error)?;
        for point in &mesh.positions {
            write!(output, "{} {} {} ", point.x, point.y, point.z).map_err(io_error)?;
        }
        writeln!(output).map_err(io_error)?;
        writeln!(
            output,
            "      </float_array><technique_common><accessor source=\"#{id}-positions-array\" count=\"{}\" stride=\"3\"><param name=\"X\" type=\"float\"/><param name=\"Y\" type=\"float\"/><param name=\"Z\" type=\"float\"/></accessor></technique_common></source>",
            mesh.positions.len()
        )
        .map_err(io_error)?;
        writeln!(
            output,
            "      <source id=\"{id}-normals\"><float_array id=\"{id}-normals-array\" count=\"{}\">",
            mesh.triangle_normals.len() * 3
        )
        .map_err(io_error)?;
        write!(output, "        ").map_err(io_error)?;
        for normal in &mesh.triangle_normals {
            write!(output, "{} {} {} ", normal.x, normal.y, normal.z).map_err(io_error)?;
        }
        writeln!(output).map_err(io_error)?;
        writeln!(
            output,
            "      </float_array><technique_common><accessor source=\"#{id}-normals-array\" count=\"{}\" stride=\"3\"><param name=\"X\" type=\"float\"/><param name=\"Y\" type=\"float\"/><param name=\"Z\" type=\"float\"/></accessor></technique_common></source>",
            mesh.triangle_normals.len()
        )
        .map_err(io_error)?;
        writeln!(
            output,
            "      <vertices id=\"{id}-vertices\"><input semantic=\"POSITION\" source=\"#{id}-positions\"/></vertices>"
        )
        .map_err(io_error)?;
        writeln!(
            output,
            "      <triangles count=\"{}\"><input semantic=\"VERTEX\" source=\"#{id}-vertices\" offset=\"0\"/><input semantic=\"NORMAL\" source=\"#{id}-normals\" offset=\"1\"/><p>",
            mesh.triangles.len()
        )
        .map_err(io_error)?;
        write!(output, "        ").map_err(io_error)?;
        for (normal, triangle) in mesh.triangles.iter().enumerate() {
            for vertex in triangle {
                write!(output, "{vertex} {normal} ").map_err(io_error)?;
            }
        }
        writeln!(output).map_err(io_error)?;
        writeln!(output, "      </p></triangles>").map_err(io_error)?;
        writeln!(output, "    </mesh></geometry>").map_err(io_error)?;
    }
    writeln!(output, "  </library_geometries>").map_err(io_error)?;
    writeln!(output, "  <library_visual_scenes>").map_err(io_error)?;
    writeln!(
        output,
        "    <visual_scene id=\"Scene\" name=\"{}\">",
        xml_escape(&collada_name(&assembly.id, "assembly"))
    )
    .map_err(io_error)?;
    let children = child_indices(assembly);
    for (index, node) in assembly.nodes.iter().enumerate() {
        if node.parent_index.is_none() {
            write_node(&mut output, assembly, &children, index, 3)?;
        }
    }
    writeln!(output, "    </visual_scene>").map_err(io_error)?;
    writeln!(output, "  </library_visual_scenes>").map_err(io_error)?;
    writeln!(
        output,
        r##"  <scene><instance_visual_scene url="#Scene"/></scene>"##
    )
    .map_err(io_error)?;
    writeln!(output, "</COLLADA>").map_err(io_error)?;
    output.flush().map_err(io_error)
}

fn child_indices(assembly: &ResolvedAssembly) -> Vec<Vec<usize>> {
    let mut children = vec![Vec::new(); assembly.nodes.len()];
    for (index, node) in assembly.nodes.iter().enumerate() {
        if let Some(parent) = node.parent_index {
            children[parent].push(index);
        }
    }
    children
}

fn write_node(
    output: &mut impl Write,
    assembly: &ResolvedAssembly,
    children: &[Vec<usize>],
    index: usize,
    depth: usize,
) -> Result<()> {
    let node = &assembly.nodes[index];
    let indent = "  ".repeat(depth);
    writeln!(
        output,
        "{indent}<node id=\"part-{}\" name=\"{}\" sid=\"part-{}\" type=\"NODE\">",
        xml_escape(&node.id),
        xml_escape(&collada_name(&node.id, "part")),
        xml_escape(&node.id)
    )
    .map_err(io_error)?;
    write!(output, "{indent}  <matrix>").map_err(io_error)?;
    // COLLADA serializes the mathematical rows in textual order. glam stores and exposes Mat4
    // arrays by columns, so transpose before flattening; otherwise translation lands in the last
    // textual row and many importers discard the transform as non-affine.
    for value in node.local_transform.transpose().to_cols_array() {
        write!(output, "{value} ").map_err(io_error)?;
    }
    writeln!(output, "</matrix>").map_err(io_error)?;
    if node.visible {
        writeln!(
            output,
            "{indent}  <instance_geometry url=\"#geometry-{}\" sid=\"mesh-{}\"/>",
            node.geometry_index,
            xml_escape(&node.id)
        )
        .map_err(io_error)?;
    }
    for child in &children[index] {
        write_node(output, assembly, children, *child, depth + 1)?;
    }
    writeln!(output, "{indent}</node>").map_err(io_error)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn collada_name(value: &str, prefix: &str) -> String {
    match value.chars().next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => value.to_string(),
        Some(_) => format!("{prefix}-{value}"),
        None => prefix.to_string(),
    }
}

fn io_error(error: std::io::Error) -> AssemblyError {
    AssemblyError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use openscad_render::{Mesh, Vec3};

    use super::*;
    use crate::{AssemblyDocument, MeshSourceRef};

    #[test]
    fn exports_shared_geometry_hierarchy_transforms_and_visibility() {
        let source = MeshSourceRef::project_source("parts/a&b.scad");
        let mesh =
            Arc::new(Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]).unwrap());
        let mut document = AssemblyDocument::new("Scene <white>");
        let root = document
            .add_part(source.clone(), "body")
            .unwrap()
            .id
            .clone();
        let child = document.add_part(source.clone(), "arm").unwrap().id.clone();
        document.part_mut(&child).unwrap().visible = false;
        document.part_mut(&child).unwrap().transform.translation = [2.0, 3.0, 4.0];
        document.set_parent(&child, Some(&root)).unwrap();
        let resolved = document.resolve(&HashMap::from([(source, mesh)])).unwrap();
        let mut output = Vec::new();

        write_dae_to(&mut output, &resolved).unwrap();

        let xml = String::from_utf8(output).unwrap();
        assert_eq!(xml.matches("<geometry id=").count(), 1);
        assert_eq!(xml.matches("<node id=").count(), 2);
        assert_eq!(xml.matches("<instance_geometry").count(), 1);
        assert!(xml.contains("<geometry id=\"geometry-0\" name=\"body-mesh\""));
        assert!(!xml.contains("<node id=\"assembly-"));
        assert!(xml.contains("<node id=\"part-body\" name=\"body\" sid=\"part-body\""));
        assert!(xml.contains("<instance_geometry url=\"#geometry-0\" sid=\"mesh-body\"/>"));
        assert!(xml.contains("1 0 0 2 0 1 0 3 0 0 1 4 0 0 0 1"));
        assert!(xml.find("part-body").unwrap() < xml.find("part-arm").unwrap());
    }

    #[test]
    fn exported_matrix_round_trips_translation_rotation_scale_and_pivot() {
        let source = MeshSourceRef::project_source("part.scad");
        let mesh =
            Arc::new(Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]).unwrap());
        let mut document = AssemblyDocument::new("matrix");
        document.add_part(source.clone(), "part").unwrap();
        let transform = &mut document.part_mut("part").unwrap().transform;
        transform.translation = [4.0, -3.0, 2.0];
        transform.rotation_degrees = [20.0, 35.0, -15.0];
        transform.scale = [2.0, 0.5, 1.5];
        transform.pivot = [1.0, 2.0, -1.0];
        let expected = transform.matrix().unwrap();
        let resolved = document.resolve(&HashMap::from([(source, mesh)])).unwrap();
        let mut output = Vec::new();
        write_dae_to(&mut output, &resolved).unwrap();
        let xml = String::from_utf8(output).unwrap();
        let matrix_text = xml
            .split_once("<matrix>")
            .unwrap()
            .1
            .split_once("</matrix>")
            .unwrap()
            .0;
        let rows = matrix_text
            .split_whitespace()
            .map(|value| value.parse::<f32>().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 16);
        let columns = [
            rows[0], rows[4], rows[8], rows[12], rows[1], rows[5], rows[9], rows[13], rows[2],
            rows[6], rows[10], rows[14], rows[3], rows[7], rows[11], rows[15],
        ];
        let restored = openscad_render::Mat4::from_cols_array(&columns);

        for point in [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z] {
            assert!(
                restored
                    .transform_point3(point)
                    .distance(expected.transform_point3(point))
                    < 1.0e-5
            );
        }

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("round-trip.dae");
        fs::write(&path, &xml).unwrap();
        let scene = openscad_render::read_model_file(path).unwrap();
        let imported = scene.instances[0].transform;
        for point in [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z] {
            assert!(
                imported
                    .transform_point3(point)
                    .distance(expected.transform_point3(point))
                    < 1.0e-5
            );
        }
    }

    #[test]
    fn exports_multiple_root_parts_without_a_synthetic_assembly_node() {
        let source = MeshSourceRef::project_source("part.scad");
        let mesh =
            Arc::new(Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]).unwrap());
        let mut document = AssemblyDocument::new("robot");
        document.add_part(source.clone(), "arm").unwrap();
        document.add_part(source.clone(), "arm").unwrap();
        let resolved = document.resolve(&HashMap::from([(source, mesh)])).unwrap();
        let mut output = Vec::new();

        write_dae_to(&mut output, &resolved).unwrap();

        let xml = String::from_utf8(output).unwrap();
        assert_eq!(xml.matches("<geometry id=").count(), 1);
        assert_eq!(xml.matches("<node id=").count(), 2);
        assert!(!xml.contains("<node id=\"assembly-"));
        assert!(xml.contains("<node id=\"part-arm\" name=\"arm\""));
        assert!(xml.contains("<node id=\"part-arm2\" name=\"arm2\""));
        assert!(xml.contains("sid=\"mesh-arm\""));
        assert!(xml.contains("sid=\"mesh-arm2\""));
    }
}
