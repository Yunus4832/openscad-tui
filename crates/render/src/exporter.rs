use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::{Mesh, RenderError, Result};

/// Write a static COLLADA 1.4.1 document containing one triangulated geometry.
///
/// OpenSCAD's unitless coordinates are identified as millimetres, matching its conventional use.
/// Scene graphs, materials, cameras, animation, and skeletal data are intentionally out of scope.
pub fn write_dae(path: impl AsRef<Path>, mesh: &Mesh) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let file = File::create(path).map_err(io_error)?;
    write_dae_to(BufWriter::new(file), mesh)
}

fn write_dae_to(mut output: impl Write, mesh: &Mesh) -> Result<()> {
    writeln!(output, r#"<?xml version="1.0" encoding="utf-8"?>"#).map_err(io_error)?;
    writeln!(
        output,
        r#"<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">"#
    )
    .map_err(io_error)?;
    writeln!(output, "  <asset>").map_err(io_error)?;
    // COLLADA requires both timestamps. A deterministic value keeps exports reproducible.
    writeln!(output, "    <created>1970-01-01T00:00:00Z</created>").map_err(io_error)?;
    writeln!(output, "    <modified>1970-01-01T00:00:00Z</modified>").map_err(io_error)?;
    writeln!(output, r#"    <unit name="millimeter" meter="0.001"/>"#).map_err(io_error)?;
    writeln!(output, "    <up_axis>Z_UP</up_axis>").map_err(io_error)?;
    writeln!(output, "  </asset>").map_err(io_error)?;
    writeln!(output, "  <library_geometries>").map_err(io_error)?;
    writeln!(
        output,
        r#"    <geometry id="openscad-mesh" name="OpenSCAD Model"><mesh>"#
    )
    .map_err(io_error)?;
    writeln!(
        output,
        r#"      <source id="openscad-mesh-positions"><float_array id="openscad-mesh-positions-array" count="{}">"#,
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
        r##"      </float_array><technique_common><accessor source="#openscad-mesh-positions-array" count="{}" stride="3"><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source>"##,
        mesh.positions.len()
    )
    .map_err(io_error)?;
    writeln!(
        output,
        r#"      <source id="openscad-mesh-normals"><float_array id="openscad-mesh-normals-array" count="{}">"#,
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
        r##"      </float_array><technique_common><accessor source="#openscad-mesh-normals-array" count="{}" stride="3"><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source>"##,
        mesh.triangle_normals.len()
    )
    .map_err(io_error)?;
    writeln!(
        output,
        r##"      <vertices id="openscad-mesh-vertices"><input semantic="POSITION" source="#openscad-mesh-positions"/></vertices>"##
    )
    .map_err(io_error)?;
    writeln!(
        output,
        r##"      <triangles count="{}"><input semantic="VERTEX" source="#openscad-mesh-vertices" offset="0"/><input semantic="NORMAL" source="#openscad-mesh-normals" offset="1"/><p>"##,
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
    writeln!(output, "  </library_geometries>").map_err(io_error)?;
    writeln!(output, "  <library_visual_scenes>").map_err(io_error)?;
    writeln!(
        output,
        r##"    <visual_scene id="Scene" name="Scene"><node id="OpenSCAD-Model" name="OpenSCAD Model"><instance_geometry url="#openscad-mesh"/></node></visual_scene>"##
    )
    .map_err(io_error)?;
    writeln!(output, "  </library_visual_scenes>").map_err(io_error)?;
    writeln!(
        output,
        r##"  <scene><instance_visual_scene url="#Scene"/></scene>"##
    )
    .map_err(io_error)?;
    writeln!(output, "</COLLADA>").map_err(io_error)?;
    output.flush().map_err(io_error)
}

fn io_error(error: std::io::Error) -> RenderError {
    RenderError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use crate::Vec3;

    use super::*;

    #[test]
    fn exports_a_static_triangle_as_collada() {
        let mesh = Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]).unwrap();
        let mut output = Vec::new();

        write_dae_to(&mut output, &mesh).unwrap();

        let xml = String::from_utf8(output).unwrap();
        assert!(xml.contains(r#"version="1.4.1""#));
        assert!(xml.contains(r#"<triangles count="1">"#));
        assert!(xml.contains("0 0 1 0 2 0"));
        assert!(xml.contains("<up_axis>Z_UP</up_axis>"));
    }

    #[test]
    fn writes_dae_to_a_nested_destination() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("exports/model.dae");
        let mesh = Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]).unwrap();

        write_dae(&destination, &mesh).unwrap();

        let xml = std::fs::read_to_string(destination).unwrap();
        assert!(xml.starts_with("<?xml"));
        assert!(xml.ends_with("</COLLADA>\n"));
    }
}
