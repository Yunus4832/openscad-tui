use crate::{Mat4, RenderError, Result, Vec3};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn from_points(points: &[Vec3]) -> Result<Self> {
        let first = *points.first().ok_or(RenderError::EmptyMesh)?;
        if !first.is_finite() {
            return Err(RenderError::NonFiniteVertex { index: 0 });
        }
        let mut min = first;
        let mut max = first;
        for (index, point) in points.iter().copied().enumerate().skip(1) {
            if !point.is_finite() {
                return Err(RenderError::NonFiniteVertex { index });
            }
            min = min.min(point);
            max = max.max(point);
        }
        Ok(Self { min, max })
    }

    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(self) -> Vec3 {
        self.max - self.min
    }

    pub fn radius(self) -> f32 {
        self.size().length() * 0.5
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn transformed(self, transform: Mat4) -> Result<Self> {
        let corners = [
            Vec3::new(self.min.x, self.min.y, self.min.z),
            Vec3::new(self.min.x, self.min.y, self.max.z),
            Vec3::new(self.min.x, self.max.y, self.min.z),
            Vec3::new(self.min.x, self.max.y, self.max.z),
            Vec3::new(self.max.x, self.min.y, self.min.z),
            Vec3::new(self.max.x, self.min.y, self.max.z),
            Vec3::new(self.max.x, self.max.y, self.min.z),
            Vec3::new(self.max.x, self.max.y, self.max.z),
        ]
        .map(|corner| transform.transform_point3(corner));
        Self::from_points(&corners)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    pub triangle_normals: Vec<Vec3>,
    pub bounds: Aabb,
}

impl Mesh {
    pub fn new(positions: Vec<Vec3>, triangles: Vec<[u32; 3]>) -> Result<Self> {
        let bounds = Aabb::from_points(&positions)?;
        let mut valid_triangles = Vec::with_capacity(triangles.len());
        let mut triangle_normals = Vec::with_capacity(triangles.len());
        for (triangle_number, triangle) in triangles.into_iter().enumerate() {
            for index in triangle {
                if index as usize >= positions.len() {
                    return Err(RenderError::InvalidTriangleIndex {
                        triangle: triangle_number,
                        index,
                        vertex_count: positions.len(),
                    });
                }
            }
            let [a, b, c] = triangle.map(|index| positions[index as usize]);
            let normal = (b - a).cross(c - a);
            if normal.length_squared() <= f32::EPSILON {
                continue;
            }
            valid_triangles.push(triangle);
            triangle_normals.push(normal.normalize());
        }
        Ok(Self {
            positions,
            triangles: valid_triangles,
            triangle_normals,
            bounds,
        })
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_cover_finite_points() {
        let bounds =
            Aabb::from_points(&[Vec3::new(-2.0, 4.0, 1.0), Vec3::new(6.0, -2.0, 5.0)]).unwrap();
        assert_eq!(bounds.min, Vec3::new(-2.0, -2.0, 1.0));
        assert_eq!(bounds.max, Vec3::new(6.0, 4.0, 5.0));
        assert_eq!(bounds.center(), Vec3::new(2.0, 1.0, 3.0));
        assert_eq!(bounds.size(), Vec3::new(8.0, 6.0, 4.0));
    }

    #[test]
    fn empty_or_non_finite_bounds_are_rejected() {
        assert_eq!(Aabb::from_points(&[]), Err(RenderError::EmptyMesh));
        assert_eq!(
            Aabb::from_points(&[Vec3::splat(f32::NAN)]),
            Err(RenderError::NonFiniteVertex { index: 0 })
        );
    }

    #[test]
    fn mesh_calculates_bounds_and_triangle_count() {
        let mesh = Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]).unwrap();
        assert_eq!(mesh.bounds.max, Vec3::new(1.0, 1.0, 0.0));
        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.triangle_normals, vec![Vec3::Z]);
    }

    #[test]
    fn mesh_rejects_invalid_indices_and_filters_degenerate_triangles() {
        assert!(matches!(
            Mesh::new(vec![Vec3::ZERO], vec![[0, 1, 0]]),
            Err(RenderError::InvalidTriangleIndex { .. })
        ));
        let mesh = Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0], vec![[0, 1, 2]]).unwrap();
        assert!(mesh.triangles.is_empty());
        assert!(mesh.triangle_normals.is_empty());
    }

    #[test]
    fn bounds_transform_and_union_cover_instances() {
        let first = Aabb::from_points(&[Vec3::ZERO, Vec3::ONE]).unwrap();
        let moved = first
            .transformed(Mat4::from_translation(Vec3::new(4.0, -2.0, 1.0)))
            .unwrap();
        assert_eq!(moved.min, Vec3::new(4.0, -2.0, 1.0));
        assert_eq!(moved.max, Vec3::new(5.0, -1.0, 2.0));
        assert_eq!(first.union(moved).max, Vec3::new(5.0, 1.0, 2.0));
    }
}
