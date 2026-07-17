use std::io::BufRead;

use crate::{Mesh, RenderError, Result, Vec3};

pub fn parse_off(source: &str) -> Result<Mesh> {
    read_off(source.as_bytes())
}

pub fn read_off(reader: impl BufRead) -> Result<Mesh> {
    let mut tokens = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|error| RenderError::InvalidOff(error.to_string()))?;
        let data = line.split_once('#').map(|(data, _)| data).unwrap_or(&line);
        tokens.extend(data.split_whitespace().map(str::to_string));
    }
    let mut input = Tokens::new(tokens);
    if input.next_string("header")? != "OFF" {
        return Err(RenderError::InvalidOff("expected OFF header".to_string()));
    }

    let vertex_count = input.next_usize("vertex count")?;
    let face_count = input.next_usize("face count")?;
    let _edge_count = input.next_usize("edge count")?;
    if vertex_count == 0 {
        return Err(RenderError::EmptyMesh);
    }

    let mut positions = Vec::with_capacity(vertex_count);
    for vertex in 0..vertex_count {
        let point = Vec3::new(
            input.next_f32(&format!("vertex {vertex} x"))?,
            input.next_f32(&format!("vertex {vertex} y"))?,
            input.next_f32(&format!("vertex {vertex} z"))?,
        );
        if !point.is_finite() {
            return Err(RenderError::NonFiniteVertex { index: vertex });
        }
        positions.push(point);
    }

    let mut triangles = Vec::new();
    for face in 0..face_count {
        let count = input.next_usize(&format!("face {face} vertex count"))?;
        if count < 3 {
            return Err(RenderError::InvalidOff(format!(
                "face {face} has fewer than three vertices"
            )));
        }
        let mut indices = Vec::with_capacity(count);
        for _ in 0..count {
            let index = input.next_u32(&format!("face {face} index"))?;
            if index as usize >= vertex_count {
                return Err(RenderError::InvalidOff(format!(
                    "face {face} index {index} is outside {vertex_count} vertices"
                )));
            }
            indices.push(index);
        }
        ensure_convex(face, &indices, &positions)?;
        for offset in 1..indices.len() - 1 {
            triangles.push([indices[0], indices[offset], indices[offset + 1]]);
        }
    }

    Mesh::new(positions, triangles)
}

fn ensure_convex(face: usize, indices: &[u32], positions: &[Vec3]) -> Result<()> {
    if indices.len() <= 3 {
        return Ok(());
    }
    let mut normal = Vec3::ZERO;
    for index in 0..indices.len() {
        let current = positions[indices[index] as usize];
        let next = positions[indices[(index + 1) % indices.len()] as usize];
        normal += current.cross(next);
    }
    if normal.length_squared() <= f32::EPSILON {
        return Ok(());
    }

    let mut winding = 0.0_f32;
    for index in 0..indices.len() {
        let a = positions[indices[index] as usize];
        let b = positions[indices[(index + 1) % indices.len()] as usize];
        let c = positions[indices[(index + 2) % indices.len()] as usize];
        let turn = (b - a).cross(c - b).dot(normal);
        if turn.abs() <= 1.0e-6 {
            continue;
        }
        if winding == 0.0 {
            winding = turn.signum();
        } else if turn.signum() != winding {
            return Err(RenderError::InvalidOff(format!(
                "face {face} is concave and cannot be triangulated by a fan"
            )));
        }
    }
    Ok(())
}

struct Tokens {
    values: Vec<String>,
    position: usize,
}

impl Tokens {
    fn new(values: Vec<String>) -> Self {
        Self {
            values,
            position: 0,
        }
    }

    fn next_string(&mut self, expected: &str) -> Result<&str> {
        let value = self.values.get(self.position).ok_or_else(|| {
            RenderError::InvalidOff(format!("missing {expected} at token {}", self.position))
        })?;
        self.position += 1;
        Ok(value)
    }

    fn next_usize(&mut self, expected: &str) -> Result<usize> {
        self.next_string(expected)?.parse().map_err(|_| {
            RenderError::InvalidOff(format!("invalid {expected} at token {}", self.position - 1))
        })
    }

    fn next_u32(&mut self, expected: &str) -> Result<u32> {
        self.next_string(expected)?.parse().map_err(|_| {
            RenderError::InvalidOff(format!("invalid {expected} at token {}", self.position - 1))
        })
    }

    fn next_f32(&mut self, expected: &str) -> Result<f32> {
        self.next_string(expected)?.parse().map_err(|_| {
            RenderError::InvalidOff(format!("invalid {expected} at token {}", self.position - 1))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_triangle_with_comments_and_blank_lines() {
        let mesh = parse_off(
            r#"
                # generated fixture
                OFF
                3 1 0

                0 0 0
                1 0 0 # inline comment
                0 1 0
                3 0 1 2
            "#,
        )
        .unwrap();
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
        assert_eq!(mesh.triangle_normals, vec![Vec3::Z]);
    }

    #[test]
    fn triangulates_a_convex_quad() {
        let mesh = parse_off("OFF\n4 1 0\n0 0 0\n1 0 0\n1 1 0\n0 1 0\n4 0 1 2 3\n").unwrap();
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn rejects_bad_header_missing_data_and_indices() {
        assert!(matches!(
            parse_off("NOFF\n"),
            Err(RenderError::InvalidOff(_))
        ));
        assert!(matches!(
            parse_off("OFF\n3 1 0\n0 0 0\n"),
            Err(RenderError::InvalidOff(_))
        ));
        assert!(matches!(
            parse_off("OFF\n3 1 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 3\n"),
            Err(RenderError::InvalidOff(_))
        ));
    }

    #[test]
    fn rejects_concave_faces() {
        let result = parse_off("OFF\n5 1 0\n0 0 0\n2 0 0\n1 1 0\n2 2 0\n0 2 0\n5 0 1 2 3 4\n");
        assert!(matches!(result, Err(RenderError::InvalidOff(_))));
    }

    #[test]
    fn filters_degenerate_faces_and_calculates_bounds() {
        let mesh =
            parse_off("OFF\n4 2 0\n-1 -2 0\n2 -2 0\n-1 3 0\n0 0 0\n3 0 1 2\n3 0 3 0\n").unwrap();
        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.bounds.min, Vec3::new(-1.0, -2.0, 0.0));
        assert_eq!(mesh.bounds.max, Vec3::new(2.0, 3.0, 0.0));
    }
}
