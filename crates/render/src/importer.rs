use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::{dae::read_dae, read_off, Mesh, RenderError, RenderScene, Result, Vec3};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFileFormat {
    Off,
    Stl,
    Dae,
}

impl ModelFileFormat {
    pub fn from_path(path: &Path) -> Result<Self> {
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("off") => Ok(Self::Off),
            Some("stl") => Ok(Self::Stl),
            Some("dae") => Ok(Self::Dae),
            _ => Err(RenderError::UnsupportedMeshFormat {
                path: path.display().to_string(),
            }),
        }
    }
}

pub fn read_mesh_file(path: impl AsRef<Path>) -> Result<Mesh> {
    let path = path.as_ref();
    match ModelFileFormat::from_path(path)? {
        ModelFileFormat::Off => {
            let file = File::open(path).map_err(io_error)?;
            read_off(BufReader::new(file))
        }
        ModelFileFormat::Stl => read_stl(path),
        ModelFileFormat::Dae => Err(RenderError::InvalidDae(
            "COLLADA contains a scene; load it through read_model_file".into(),
        )),
    }
}

pub fn read_model_file(path: impl AsRef<Path>) -> Result<RenderScene> {
    let path = path.as_ref();
    match ModelFileFormat::from_path(path)? {
        ModelFileFormat::Off | ModelFileFormat::Stl => {
            read_mesh_file(path).map(RenderScene::single)
        }
        ModelFileFormat::Dae => read_dae(path),
    }
}

fn read_stl(path: &Path) -> Result<Mesh> {
    let file = File::open(path).map_err(io_error)?;
    let indexed = stl_io::read_stl(&mut BufReader::new(file))
        .map_err(|error| RenderError::InvalidStl(error.to_string()))?;
    let positions = indexed
        .vertices
        .into_iter()
        .map(|vertex| Vec3::new(vertex[0], vertex[1], vertex[2]))
        .collect();
    let triangles = indexed
        .faces
        .into_iter()
        .enumerate()
        .map(|(triangle, face)| {
            let convert = |index| {
                u32::try_from(index).map_err(|_| {
                    RenderError::InvalidStl(format!(
                        "triangle {triangle} vertex index {index} exceeds u32"
                    ))
                })
            };
            Ok([
                convert(face.vertices[0])?,
                convert(face.vertices[1])?,
                convert(face.vertices[2])?,
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    Mesh::new(positions, triangles)
}

fn io_error(error: std::io::Error) -> RenderError {
    RenderError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn detects_supported_model_extensions_case_insensitively() {
        assert_eq!(
            ModelFileFormat::from_path(Path::new("part.OFF")),
            Ok(ModelFileFormat::Off)
        );
        assert_eq!(
            ModelFileFormat::from_path(Path::new("part.StL")),
            Ok(ModelFileFormat::Stl)
        );
        assert_eq!(
            ModelFileFormat::from_path(Path::new("part.DAE")),
            Ok(ModelFileFormat::Dae)
        );
    }

    #[test]
    fn reads_off_and_ascii_stl_files() {
        let directory = tempfile::tempdir().unwrap();
        let off = directory.path().join("triangle.off");
        std::fs::write(&off, "OFF\n3 1 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n").unwrap();
        assert_eq!(read_mesh_file(off).unwrap().triangle_count(), 1);

        let stl = directory.path().join("triangle.stl");
        let mut file = File::create(&stl).unwrap();
        file.write_all(
            b"solid triangle\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid triangle\n",
        )
        .unwrap();
        assert_eq!(read_mesh_file(stl).unwrap().triangle_count(), 1);
    }

    #[test]
    fn reads_binary_stl_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("triangle.stl");
        let mut bytes = vec![0_u8; 80];
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        for value in [
            0.0_f32, 0.0, 1.0, // normal
            0.0, 0.0, 0.0, // first vertex
            1.0, 0.0, 0.0, // second vertex
            0.0, 1.0, 0.0, // third vertex
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();

        let mesh = read_mesh_file(path).unwrap();

        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.triangle_normals, vec![Vec3::Z]);
    }
}
