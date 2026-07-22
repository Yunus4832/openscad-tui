use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use roxmltree::{Document, Node};

use crate::{Mat4, Mesh, RenderError, RenderInstance, RenderScene, Result, Vec3};

pub(crate) fn read_dae(path: &Path) -> Result<RenderScene> {
    let xml = fs::read_to_string(path).map_err(io_error)?;
    parse_dae(&xml)
}

fn parse_dae(xml: &str) -> Result<RenderScene> {
    let document = Document::parse(xml).map_err(|error| invalid(error.to_string()))?;
    let root = document.root_element();
    if !root.has_tag_name("COLLADA") {
        return Err(invalid("root element is not COLLADA"));
    }

    let mut meshes = Vec::new();
    let mut geometry_indices = HashMap::new();
    for geometry in root
        .descendants()
        .filter(|node| node.has_tag_name("geometry"))
    {
        let Some(id) = geometry.attribute("id") else {
            return Err(invalid("geometry is missing an id"));
        };
        let mesh = parse_geometry(geometry, id)?;
        geometry_indices.insert(id.to_string(), meshes.len());
        meshes.push(Arc::new(mesh));
    }
    if meshes.is_empty() {
        return Err(invalid("document contains no mesh geometry"));
    }

    let visual_scene = selected_visual_scene(root)?;
    let mut instances = Vec::new();
    let root_transform = up_axis_transform(root)?;
    for node in visual_scene
        .children()
        .filter(|node| node.has_tag_name("node"))
    {
        collect_node_instances(node, root_transform, &geometry_indices, &mut instances)?;
    }
    if instances.is_empty() {
        return Err(invalid(
            "selected visual scene contains no geometry instances",
        ));
    }
    RenderScene::new(meshes, instances)
}

fn parse_geometry(geometry: Node<'_, '_>, geometry_id: &str) -> Result<Mesh> {
    let mesh = geometry
        .children()
        .find(|node| node.has_tag_name("mesh"))
        .ok_or_else(|| invalid(format!("geometry '{geometry_id}' is not a mesh")))?;
    let vertices_sources = mesh
        .children()
        .filter(|node| node.has_tag_name("vertices"))
        .filter_map(|vertices| {
            let id = vertices.attribute("id")?;
            let source = vertices
                .children()
                .find(|input| {
                    input.has_tag_name("input") && input.attribute("semantic") == Some("POSITION")
                })?
                .attribute("source")?;
            Some((id.to_string(), fragment(source).ok()?.to_string()))
        })
        .collect::<HashMap<_, _>>();

    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    let mut found_primitive = false;
    for primitive in mesh.children().filter(|node| {
        node.has_tag_name("triangles")
            || node.has_tag_name("polylist")
            || node.has_tag_name("polygons")
    }) {
        found_primitive = true;
        let inputs = primitive
            .children()
            .filter(|node| node.has_tag_name("input"))
            .collect::<Vec<_>>();
        let stride = inputs
            .iter()
            .map(|input| parse_usize_attr(*input, "offset", 0))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max()
            .map(|offset| offset + 1)
            .ok_or_else(|| invalid(format!("geometry '{geometry_id}' has no primitive inputs")))?;
        let vertex_input = inputs
            .iter()
            .find(|input| matches!(input.attribute("semantic"), Some("VERTEX" | "POSITION")))
            .ok_or_else(|| {
                invalid(format!(
                    "geometry '{geometry_id}' primitive has no VERTEX input"
                ))
            })?;
        let vertex_offset = parse_usize_attr(*vertex_input, "offset", 0)?;
        let input_source = fragment(
            vertex_input
                .attribute("source")
                .ok_or_else(|| invalid("primitive input is missing a source"))?,
        )?;
        let position_source = if vertex_input.attribute("semantic") == Some("VERTEX") {
            vertices_sources.get(input_source).ok_or_else(|| {
                invalid(format!(
                    "geometry '{geometry_id}' references unknown vertices '{input_source}'"
                ))
            })?
        } else {
            input_source
        };
        let primitive_positions = parse_positions(mesh, position_source)?;
        let primitive_position_count = primitive_positions.len();
        let position_base = u32::try_from(positions.len())
            .map_err(|_| invalid("COLLADA vertex count exceeds u32"))?;
        positions.extend(primitive_positions);

        if primitive.has_tag_name("triangles") {
            for p in primitive.children().filter(|node| node.has_tag_name("p")) {
                let indices = parse_usizes(p.text().unwrap_or_default(), "triangle indices")?;
                let records = position_records(
                    &indices,
                    stride,
                    vertex_offset,
                    position_base,
                    primitive_position_count,
                )?;
                if !records.len().is_multiple_of(3) {
                    return Err(invalid(format!(
                        "geometry '{geometry_id}' triangle index count is not divisible by 3"
                    )));
                }
                triangles.extend(
                    records
                        .chunks_exact(3)
                        .map(|chunk| [chunk[0], chunk[1], chunk[2]]),
                );
            }
        } else if primitive.has_tag_name("polylist") {
            let counts = primitive
                .children()
                .find(|node| node.has_tag_name("vcount"))
                .ok_or_else(|| invalid("polylist is missing vcount"))?;
            let counts = parse_usizes(counts.text().unwrap_or_default(), "polygon sizes")?;
            let indices = primitive
                .children()
                .filter(|node| node.has_tag_name("p"))
                .flat_map(|node| node.text().unwrap_or_default().split_whitespace())
                .map(|value| {
                    value
                        .parse::<usize>()
                        .map_err(|_| invalid(format!("invalid polygon index '{value}'")))
                })
                .collect::<Result<Vec<_>>>()?;
            let records = position_records(
                &indices,
                stride,
                vertex_offset,
                position_base,
                primitive_position_count,
            )?;
            triangulate_polygons(&records, &counts, &mut triangles)?;
        } else {
            for p in primitive.children().filter(|node| node.has_tag_name("p")) {
                let indices = parse_usizes(p.text().unwrap_or_default(), "polygon indices")?;
                let records = position_records(
                    &indices,
                    stride,
                    vertex_offset,
                    position_base,
                    primitive_position_count,
                )?;
                triangulate_polygons(&records, &[records.len()], &mut triangles)?;
            }
        }
    }
    if !found_primitive || triangles.is_empty() {
        return Err(invalid(format!(
            "geometry '{geometry_id}' contains no triangle-compatible primitives"
        )));
    }
    Mesh::new(positions, triangles)
}

fn parse_positions(mesh: Node<'_, '_>, source_id: &str) -> Result<Vec<Vec3>> {
    let source = mesh
        .children()
        .find(|node| node.has_tag_name("source") && node.attribute("id") == Some(source_id))
        .ok_or_else(|| invalid(format!("position source '{source_id}' was not found")))?;
    let accessor = source
        .descendants()
        .find(|node| node.has_tag_name("accessor"))
        .ok_or_else(|| invalid(format!("position source '{source_id}' has no accessor")))?;
    let array_id = accessor
        .attribute("source")
        .map(fragment)
        .transpose()?
        .unwrap_or_default();
    let float_array = source
        .children()
        .find(|node| {
            node.has_tag_name("float_array")
                && (array_id.is_empty() || node.attribute("id") == Some(array_id))
        })
        .ok_or_else(|| invalid(format!("position source '{source_id}' has no float_array")))?;
    let values = parse_f32s(float_array.text().unwrap_or_default(), "position values")?;
    let stride = parse_usize_attr(accessor, "stride", 1)?;
    let offset = parse_usize_attr(accessor, "offset", 0)?;
    let count = accessor
        .attribute("count")
        .map(|value| parse_usize(value, "accessor count"))
        .transpose()?
        .unwrap_or_else(|| values.len().saturating_sub(offset) / stride);
    let params = accessor
        .children()
        .filter(|node| node.has_tag_name("param"))
        .enumerate()
        .filter_map(|(index, param)| param.attribute("name").map(|name| (name, index)))
        .collect::<HashMap<_, _>>();
    let components = [
        params.get("X").copied().unwrap_or(0),
        params.get("Y").copied().unwrap_or(1),
        params.get("Z").copied().unwrap_or(2),
    ];
    if stride == 0 || components.iter().any(|component| *component >= stride) {
        return Err(invalid(format!(
            "position source '{source_id}' has an invalid accessor stride"
        )));
    }
    let mut positions = Vec::with_capacity(count);
    for row in 0..count {
        let start = offset
            .checked_add(row.saturating_mul(stride))
            .ok_or_else(|| invalid("position accessor offset overflow"))?;
        let component = |index: usize| {
            values
                .get(start + components[index])
                .copied()
                .ok_or_else(|| {
                    invalid(format!(
                        "position source '{source_id}' accessor exceeds its array"
                    ))
                })
        };
        positions.push(Vec3::new(component(0)?, component(1)?, component(2)?));
    }
    Ok(positions)
}

fn position_records(
    indices: &[usize],
    stride: usize,
    vertex_offset: usize,
    position_base: u32,
    position_count: usize,
) -> Result<Vec<u32>> {
    if stride == 0 || !indices.len().is_multiple_of(stride) {
        return Err(invalid(
            "primitive index list does not match its input stride",
        ));
    }
    indices
        .chunks_exact(stride)
        .map(|record| {
            let index = *record
                .get(vertex_offset)
                .ok_or_else(|| invalid("VERTEX offset exceeds primitive input stride"))?;
            if index >= position_count {
                return Err(invalid(format!(
                    "primitive references position {index}, but its source contains only {position_count}"
                )));
            }
            let index =
                u32::try_from(index).map_err(|_| invalid("COLLADA vertex index exceeds u32"))?;
            position_base
                .checked_add(index)
                .ok_or_else(|| invalid("COLLADA vertex index exceeds u32"))
        })
        .collect()
}

fn triangulate_polygons(
    records: &[u32],
    counts: &[usize],
    output: &mut Vec<[u32; 3]>,
) -> Result<()> {
    let mut start: usize = 0;
    for count in counts {
        let end = start
            .checked_add(*count)
            .ok_or_else(|| invalid("polygon vertex count overflow"))?;
        let polygon = records
            .get(start..end)
            .ok_or_else(|| invalid("polygon sizes exceed the supplied indices"))?;
        if polygon.len() < 3 {
            return Err(invalid("polygon contains fewer than three vertices"));
        }
        for index in 1..polygon.len() - 1 {
            output.push([polygon[0], polygon[index], polygon[index + 1]]);
        }
        start = end;
    }
    if start != records.len() {
        return Err(invalid("polygon sizes do not consume all supplied indices"));
    }
    Ok(())
}

fn selected_visual_scene<'a, 'input>(root: Node<'a, 'input>) -> Result<Node<'a, 'input>> {
    let requested = root
        .children()
        .find(|node| node.has_tag_name("scene"))
        .and_then(|scene| {
            scene
                .children()
                .find(|node| node.has_tag_name("instance_visual_scene"))
        })
        .and_then(|instance| instance.attribute("url"))
        .map(fragment)
        .transpose()?;
    root.descendants()
        .filter(|node| node.has_tag_name("visual_scene"))
        .find(|scene| requested.is_none() || scene.attribute("id") == requested)
        .ok_or_else(|| invalid("selected visual_scene was not found"))
}

fn collect_node_instances(
    node: Node<'_, '_>,
    parent_transform: Mat4,
    geometry_indices: &HashMap<String, usize>,
    instances: &mut Vec<RenderInstance>,
) -> Result<()> {
    let world_transform = parent_transform * node_transform(node)?;
    for instance in node
        .children()
        .filter(|child| child.has_tag_name("instance_geometry"))
    {
        let geometry_id = fragment(
            instance
                .attribute("url")
                .ok_or_else(|| invalid("instance_geometry is missing a url"))?,
        )?;
        let mesh_index = geometry_indices.get(geometry_id).copied().ok_or_else(|| {
            invalid(format!(
                "instance_geometry references unknown geometry '{geometry_id}'"
            ))
        })?;
        let mut render_instance = RenderInstance::new(mesh_index, world_transform);
        render_instance.object_id = u32::try_from(instances.len() + 1)
            .map_err(|_| invalid("COLLADA instance count exceeds u32"))?;
        instances.push(render_instance);
    }
    if node
        .children()
        .any(|child| child.has_tag_name("instance_controller"))
    {
        return Err(invalid(
            "skinned controller instances are not supported by static DAE preview",
        ));
    }
    if node
        .children()
        .any(|child| child.has_tag_name("instance_node"))
    {
        return Err(invalid(
            "instance_node is not supported by static DAE preview",
        ));
    }
    for child in node.children().filter(|child| child.has_tag_name("node")) {
        collect_node_instances(child, world_transform, geometry_indices, instances)?;
    }
    Ok(())
}

fn node_transform(node: Node<'_, '_>) -> Result<Mat4> {
    let mut transform = Mat4::IDENTITY;
    for element in node.children().filter(Node::is_element) {
        let next = if element.has_tag_name("matrix") {
            let values = parse_f32s(element.text().unwrap_or_default(), "node matrix")?;
            let values: [f32; 16] = values
                .try_into()
                .map_err(|_| invalid("node matrix must contain exactly 16 values"))?;
            Mat4::from_cols_array(&values).transpose()
        } else if element.has_tag_name("translate") {
            Mat4::from_translation(parse_vec3(element, "translation")?)
        } else if element.has_tag_name("rotate") {
            let values = parse_f32s(element.text().unwrap_or_default(), "rotation")?;
            if values.len() != 4 {
                return Err(invalid("rotation must contain an axis and angle"));
            }
            let axis = Vec3::new(values[0], values[1], values[2]);
            if axis.length_squared() <= f32::EPSILON {
                return Err(invalid("rotation axis must be non-zero"));
            }
            Mat4::from_axis_angle(axis.normalize(), values[3].to_radians())
        } else if element.has_tag_name("scale") {
            Mat4::from_scale(parse_vec3(element, "scale")?)
        } else if element.has_tag_name("lookat") || element.has_tag_name("skew") {
            return Err(invalid(format!(
                "{} transforms are not supported by static DAE preview",
                element.tag_name().name()
            )));
        } else {
            continue;
        };
        transform *= next;
    }
    if !transform.is_finite() {
        return Err(invalid("node transform contains non-finite values"));
    }
    Ok(transform)
}

fn up_axis_transform(root: Node<'_, '_>) -> Result<Mat4> {
    let axis = root
        .children()
        .find(|node| node.has_tag_name("asset"))
        .and_then(|asset| asset.children().find(|node| node.has_tag_name("up_axis")))
        .and_then(|node| node.text())
        .map(str::trim)
        .unwrap_or("Y_UP");
    match axis {
        "Z_UP" => Ok(Mat4::IDENTITY),
        "Y_UP" => Ok(Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        "X_UP" => Ok(Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
        value => Err(invalid(format!("unsupported up_axis '{value}'"))),
    }
}

fn parse_vec3(node: Node<'_, '_>, label: &str) -> Result<Vec3> {
    let values = parse_f32s(node.text().unwrap_or_default(), label)?;
    if values.len() != 3 {
        return Err(invalid(format!("{label} must contain exactly 3 values")));
    }
    Ok(Vec3::new(values[0], values[1], values[2]))
}

fn parse_f32s(text: &str, label: &str) -> Result<Vec<f32>> {
    text.split_whitespace()
        .map(|value| {
            value
                .parse::<f32>()
                .map_err(|_| invalid(format!("invalid {label} value '{value}'")))
        })
        .collect()
}

fn parse_usizes(text: &str, label: &str) -> Result<Vec<usize>> {
    text.split_whitespace()
        .map(|value| parse_usize(value, label))
        .collect()
}

fn parse_usize(value: &str, label: &str) -> Result<usize> {
    value
        .parse::<usize>()
        .map_err(|_| invalid(format!("invalid {label} value '{value}'")))
}

fn parse_usize_attr(node: Node<'_, '_>, name: &str, default: usize) -> Result<usize> {
    node.attribute(name)
        .map(|value| parse_usize(value, name))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn fragment(value: &str) -> Result<&str> {
    value
        .strip_prefix('#')
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(format!("external or empty URI '{value}' is not supported")))
}

fn invalid(message: impl Into<String>) -> RenderError {
    RenderError::InvalidDae(message.into())
}

fn io_error(error: std::io::Error) -> RenderError {
    RenderError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = r##"
        <COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
          <asset><up_axis>Z_UP</up_axis></asset>
          <library_geometries>
            <geometry id="triangle"><mesh>
              <source id="positions">
                <float_array id="positions-array" count="9">0 0 0 1 0 0 0 1 0</float_array>
                <technique_common><accessor source="#positions-array" count="3" stride="3">
                  <param name="X"/><param name="Y"/><param name="Z"/>
                </accessor></technique_common>
              </source>
              <vertices id="vertices"><input semantic="POSITION" source="#positions"/></vertices>
              <triangles count="1"><input semantic="VERTEX" source="#vertices" offset="0"/><p>0 1 2</p></triangles>
            </mesh></geometry>
          </library_geometries>
          <library_visual_scenes><visual_scene id="Scene">
            <node id="root"><translate>2 3 4</translate><instance_geometry url="#triangle"/>
              <node id="child"><matrix>1 0 0 5 0 1 0 0 0 0 1 0 0 0 0 1</matrix><instance_geometry url="#triangle"/></node>
            </node>
          </visual_scene></library_visual_scenes>
          <scene><instance_visual_scene url="#Scene"/></scene>
        </COLLADA>
    "##;

    #[test]
    fn parses_static_scene_geometry_instances_and_hierarchy() {
        let scene = parse_dae(SCENE).unwrap();

        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.instances.len(), 2);
        assert_eq!(scene.triangle_count(), 2);
        assert_eq!(
            scene.instances[0].transform.transform_point3(Vec3::ZERO),
            Vec3::new(2.0, 3.0, 4.0)
        );
        assert_eq!(
            scene.instances[1].transform.transform_point3(Vec3::ZERO),
            Vec3::new(7.0, 3.0, 4.0)
        );
    }

    #[test]
    fn triangulates_polylist_and_converts_y_up() {
        let scene = parse_dae(&SCENE.replace(
            "<asset><up_axis>Z_UP</up_axis></asset>",
            "<asset><up_axis>Y_UP</up_axis></asset>",
        )
        .replace(
            "<triangles count=\"1\"><input semantic=\"VERTEX\" source=\"#vertices\" offset=\"0\"/><p>0 1 2</p></triangles>",
            "<polylist count=\"1\"><input semantic=\"VERTEX\" source=\"#vertices\" offset=\"0\"/><vcount>3</vcount><p>0 1 2</p></polylist>",
        ))
        .unwrap();

        assert_eq!(scene.triangle_count(), 2);
        let origin = scene.instances[0].transform.transform_point3(Vec3::ZERO);
        assert!((origin - Vec3::new(2.0, -4.0, 3.0)).length() < 1e-5);
    }

    #[test]
    fn rejects_controller_instances_with_an_actionable_error() {
        let xml = SCENE.replace(
            "<instance_geometry url=\"#triangle\"/>",
            "<instance_controller url=\"#skin\"/>",
        );
        let error = parse_dae(&xml).unwrap_err();

        assert!(error.to_string().contains("controller"));
        assert!(error.to_string().contains("static DAE preview"));
    }
}
