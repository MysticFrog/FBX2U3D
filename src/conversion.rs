use std::{
    collections::{HashMap, HashSet},
    env,
    fmt::Write as _,
    fs::{self, File},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use fbx::{File as FbxFile, Node as FbxNode, Property as FbxProperty};
use tempfile::tempdir;

use crate::{
    cli::{Backend, Cli},
    error::AppError,
};

const BINARY_FBX_MAGIC_PREFIX: &[u8] = b"Kaydara FBX Binary";
const LOCAL_U3D_CONVERTER_SUBPATH: &str =
    "fbx2u3d\\u3d-sdk\\U3D_A_061228_5\\Bin\\Win32\\Release\\IDTFConverter.exe";
const BUNDLED_U3D_CONVERTER_SUBPATH: &str =
    "u3d-sdk\\U3D_A_061228_5\\Bin\\Win32\\Release\\IDTFConverter.exe";
const FBX2U3D_NOTICE_NODE_NAME: &str =
    "FBX2U3D | https://github.com/MysticFrog/FBX2U3D | fbx2u3d@monkeyco.net | non-commercial output";
const FBX2U3D_NOTICE_MESH_NAME: &str = "FBX2U3D_NoticeMesh";
const FBX2U3D_NOTICE_SHADER_NAME: &str = "FBX2U3D_NoticeShader";
const FBX2U3D_NOTICE_MATERIAL_NAME: &str = "FBX2U3D_NoticeMaterial";
const U3D_MAX_RESOURCE_QUALITY: &str = "1000";

#[derive(Debug, Clone, PartialEq)]
pub struct ConversionPlan {
    input: PathBuf,
    output: PathBuf,
    backend: Backend,
    units_scale: f32,
    dry_run: bool,
    idtf_converter: PathBuf,
}

impl ConversionPlan {
    fn summary(&self) -> String {
        format!(
            "input: {}\noutput: {}\nbackend: {}\nunits scale: {}\nconverter: {}\nmode: {}",
            self.input.display(),
            self.output.display(),
            self.backend.as_str(),
            self.units_scale,
            self.idtf_converter.display(),
            if self.dry_run { "dry-run" } else { "convert" }
        )
    }
}

pub fn build_plan(cli: &Cli) -> Result<ConversionPlan, AppError> {
    if !cli.units_scale.is_finite() || cli.units_scale <= 0.0 {
        return Err(AppError::InvalidUnitsScale(cli.units_scale));
    }

    if !cli.input.is_file() {
        return Err(AppError::InputMissing(cli.input.clone()));
    }

    if !is_fbx_file(&cli.input) {
        return Err(AppError::UnsupportedInput(cli.input.clone()));
    }

    let input = cli.input.canonicalize()?;
    let output = resolve_output_path(&input, cli.output.as_deref());
    let idtf_converter = resolve_idtf_converter(cli.idtf_converter.as_deref())?;

    if !is_u3d_file(&output) {
        return Err(AppError::UnsupportedOutput(output));
    }

    if let Some(parent) = output.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        if !parent.exists() {
            return Err(AppError::OutputDirectoryMissing(parent.to_path_buf()));
        }
    }

    if output.exists() && !cli.overwrite {
        return Err(AppError::OutputExists(output));
    }

    Ok(ConversionPlan {
        input,
        output,
        backend: cli.backend,
        units_scale: cli.units_scale,
        dry_run: cli.dry_run,
        idtf_converter,
    })
}

pub fn execute(plan: &ConversionPlan) -> Result<String, AppError> {
    let scene = load_scene_mesh(&plan.input, f64::from(plan.units_scale))?;

    if plan.dry_run {
        return Ok(format!(
            "Conversion plan ready.\n{}\nscene nodes: {}\nmesh parts: {}\nmesh vertices: {}\nmesh triangles: {}\nmaterials: {}\ntextured shaders: {}",
            plan.summary(),
            scene.nodes.len(),
            scene.parts.len(),
            scene.vertex_count(),
            scene.triangle_count(),
            scene.shading_count(),
            scene.textured_shader_count()
        ));
    }

    match plan.backend {
        Backend::Idtf => execute_idtf_backend(plan, &scene),
    }
}

fn execute_idtf_backend(plan: &ConversionPlan, scene: &SceneMesh) -> Result<String, AppError> {
    let temp_dir = tempdir()?;
    let idtf_path = temp_dir.path().join("scene.idtf");
    let mut staged_scene = stage_scene_assets(scene, temp_dir.path())?;
    ensure_unique_scene_resource_names(&mut staged_scene);

    write_idtf_document(&idtf_path, &staged_scene, &plan.input)?;

    if plan.output.exists() {
        fs::remove_file(&plan.output)?;
    }

    run_idtf_converter(&plan.idtf_converter, &idtf_path, &plan.output)?;

    Ok(format!(
        "Converted {} to {} using IDTFConverter.exe (nodes: {}, parts: {}, vertices: {}, triangles: {}, materials: {}, textured shaders: {}).",
        plan.input.display(),
        plan.output.display(),
        scene.nodes.len(),
        scene.parts.len(),
        scene.vertex_count(),
        scene.triangle_count(),
        scene.shading_count(),
        scene.textured_shader_count()
    ))
}

fn resolve_output_path(input: &Path, explicit_output: Option<&Path>) -> PathBuf {
    match explicit_output {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => input
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path),
        None => input.with_extension("u3d"),
    }
}

fn resolve_idtf_converter(explicit: Option<&Path>) -> Result<PathBuf, AppError> {
    let mut candidates = Vec::new();

    if let Some(path) = explicit {
        candidates.push(path.to_path_buf());
    }

    if let Some(path) = env::var_os("U3D_IDTF_CONVERTER") {
        candidates.push(PathBuf::from(path));
    }

    if let Ok(current_exe) = env::current_exe() {
        if let Some(path) = bundled_idtf_converter_path(&current_exe) {
            candidates.push(path);
        }
    }

    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local_app_data).join(LOCAL_U3D_CONVERTER_SUBPATH));
    }

    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }

    Err(AppError::IdtfConverterMissing(
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
    ))
}

fn bundled_idtf_converter_path(executable_path: &Path) -> Option<PathBuf> {
    executable_path
        .parent()
        .map(|directory| directory.join(BUNDLED_U3D_CONVERTER_SUBPATH))
}

fn is_fbx_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("fbx"))
}

fn is_u3d_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("u3d"))
}

fn load_scene_mesh(path: &Path, units_scale: f64) -> Result<SceneMesh, AppError> {
    ensure_binary_fbx(path)?;

    let reader = BufReader::new(File::open(path)?);
    let file = FbxFile::read_from(reader).map_err(|error| AppError::FbxParse(error.to_string()))?;

    extract_scene_mesh(&file, path, units_scale)
}

fn ensure_binary_fbx(path: &Path) -> Result<(), AppError> {
    let mut header = [0_u8; 21];
    let mut file = File::open(path)?;
    file.read_exact(&mut header)?;

    if header.starts_with(BINARY_FBX_MAGIC_PREFIX) {
        return Ok(());
    }

    Err(AppError::UnsupportedFbxFlavor(
        "only binary FBX 7.x files are supported by the current backend".to_owned(),
    ))
}

fn extract_scene_mesh(file: &FbxFile, source_path: &Path, units_scale: f64) -> Result<SceneMesh, AppError> {
    let objects = find_node(&file.children, "Objects").ok_or(AppError::MeshMissing)?;

    let mut geometries = HashMap::new();
    let mut models = HashMap::new();
    let mut materials = HashMap::new();
    let mut textures = HashMap::new();
    let mut videos = HashMap::new();

    for child in &objects.children {
        match child.name.as_str() {
            "Geometry" => {
                if let Some(geometry) = parse_geometry(child)? {
                    geometries.insert(geometry.id, geometry);
                }
            }
            "Model" => {
                if let Some(model) = parse_model(child)? {
                    models.insert(model.id, model);
                }
            }
            "Material" => {
                if let Some(material) = parse_material(child)? {
                    materials.insert(material.id, material);
                }
            }
            "Texture" => {
                if let Some(texture) = parse_texture(child)? {
                    textures.insert(texture.id, texture);
                }
            }
            "Video" => {
                if let Some(video) = parse_video(child)? {
                    videos.insert(video.id, video);
                }
            }
            _ => {}
        }
    }

    if geometries.is_empty() {
        return Err(AppError::MeshMissing);
    }

    let connections = find_node(&file.children, "Connections");
    let connections = parse_connections(connections, &geometries, &models, &materials, &textures, &videos)?;

    for (&model_id, &parent_id) in &connections.model_to_parent {
        if let Some(model) = models.get_mut(&model_id) {
            model.parent_model = Some(parent_id);
        }
    }

    let mut scene = SceneMesh {
        bounds: Bounds::empty(),
        nodes: Vec::new(),
        parts: Vec::new(),
    };

    let mut geometry_ids_by_model = HashMap::<i64, Vec<i64>>::new();
    for (&geometry_id, model_ids) in &connections.geometry_to_models {
        for &model_id in model_ids {
            geometry_ids_by_model.entry(model_id).or_default().push(geometry_id);
        }
    }
    for geometry_ids in geometry_ids_by_model.values_mut() {
        geometry_ids.sort_unstable();
    }

    let mut model_node_indices = HashMap::<i64, usize>::new();
    for model_id in ordered_model_ids(&models)? {
        let model = models.get(&model_id).ok_or_else(|| {
            AppError::UnsupportedFbxFeature(format!("model id {model_id} disappeared during extraction"))
        })?;
        let parent_index = model
            .parent_model
            .and_then(|parent_id| model_node_indices.get(&parent_id).copied());
        let geometry_ids = geometry_ids_by_model.get(&model_id).cloned().unwrap_or_default();

        let node_index = scene.nodes.len();
        scene.nodes.push(SceneNode {
            name: model.name.clone(),
            parent_index,
            transform: model.transform_matrix(units_scale),
            mesh_index: None,
        });
        model_node_indices.insert(model_id, node_index);

        match geometry_ids.as_slice() {
            [] => {}
            [geometry_id] => {
                let geometry = geometries.get(geometry_id).ok_or_else(|| {
                    AppError::UnsupportedFbxFeature(format!(
                        "geometry id {} referenced by model {} was not found",
                        geometry_id, model_id
                    ))
                })?;
                include_geometry_bounds(&mut scene.bounds, geometry, Some(model_id), &models, units_scale)?;

                let mesh_index = scene.parts.len();
                scene.parts.push(build_scene_part(
                    geometry,
                    Some(model_id),
                    node_index,
                    &model.name,
                    &materials,
                    &connections,
                    &textures,
                    &videos,
                    source_path,
                    units_scale,
                )?);
                scene.nodes[node_index].mesh_index = Some(mesh_index);
            }
            _ => {
                for (geometry_ordinal, geometry_id) in geometry_ids.iter().enumerate() {
                    let geometry = geometries.get(geometry_id).ok_or_else(|| {
                        AppError::UnsupportedFbxFeature(format!(
                            "geometry id {} referenced by model {} was not found",
                            geometry_id, model_id
                        ))
                    })?;
                    include_geometry_bounds(&mut scene.bounds, geometry, Some(model_id), &models, units_scale)?;

                    let body_name = if geometry.name.trim().is_empty() {
                        format!("{}_Body{}", model.name, geometry_ordinal + 1)
                    } else {
                        geometry.name.clone()
                    };
                    let body_node_index = scene.nodes.len();
                    scene.nodes.push(SceneNode {
                        name: body_name.clone(),
                        parent_index: Some(node_index),
                        transform: TransformMatrix::identity(),
                        mesh_index: None,
                    });

                    let mesh_index = scene.parts.len();
                    scene.parts.push(build_scene_part(
                        geometry,
                        Some(model_id),
                        body_node_index,
                        &body_name,
                        &materials,
                        &connections,
                        &textures,
                        &videos,
                        source_path,
                        units_scale,
                    )?);
                    scene.nodes[body_node_index].mesh_index = Some(mesh_index);
                }
            }
        }
    }

    let mut unbound_geometry_ids = geometries
        .keys()
        .copied()
        .filter(|geometry_id| !connections.geometry_to_models.contains_key(geometry_id))
        .collect::<Vec<_>>();
    unbound_geometry_ids.sort_unstable();

    for (geometry_ordinal, geometry_id) in unbound_geometry_ids.iter().enumerate() {
        let geometry = geometries.get(geometry_id).ok_or_else(|| {
            AppError::UnsupportedFbxFeature(format!("geometry id {geometry_id} could not be loaded"))
        })?;
        include_geometry_bounds(&mut scene.bounds, geometry, None, &models, units_scale)?;

        let node_name = if geometry.name.trim().is_empty() {
            format!("Body{}", geometry_ordinal + 1)
        } else {
            geometry.name.clone()
        };
        let node_index = scene.nodes.len();
        scene.nodes.push(SceneNode {
            name: node_name.clone(),
            parent_index: None,
            transform: TransformMatrix::identity(),
            mesh_index: None,
        });

        let mesh_index = scene.parts.len();
        scene.parts.push(build_scene_part(
            geometry,
            None,
            node_index,
            &node_name,
            &materials,
            &connections,
            &textures,
            &videos,
            source_path,
            units_scale,
        )?);
        scene.nodes[node_index].mesh_index = Some(mesh_index);
    }

    if scene.parts.is_empty() {
        return Err(AppError::MeshMissing);
    }

    Ok(scene)
}

fn ordered_model_ids(models: &HashMap<i64, Model>) -> Result<Vec<i64>, AppError> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum VisitState {
        Visiting,
        Done,
    }

    fn visit_model(
        model_id: i64,
        models: &HashMap<i64, Model>,
        visit_state: &mut HashMap<i64, VisitState>,
        ordered_ids: &mut Vec<i64>,
    ) -> Result<(), AppError> {
        match visit_state.get(&model_id) {
            Some(VisitState::Done) => return Ok(()),
            Some(VisitState::Visiting) => {
                return Err(AppError::UnsupportedFbxFeature(
                    "model hierarchy is cyclic".to_owned(),
                ))
            }
            None => {}
        }

        visit_state.insert(model_id, VisitState::Visiting);
        if let Some(parent_id) = models.get(&model_id).and_then(|model| model.parent_model) {
            if models.contains_key(&parent_id) {
                visit_model(parent_id, models, visit_state, ordered_ids)?;
            }
        }
        visit_state.insert(model_id, VisitState::Done);
        ordered_ids.push(model_id);
        Ok(())
    }

    let mut model_ids = models.keys().copied().collect::<Vec<_>>();
    model_ids.sort_unstable();

    let mut visit_state = HashMap::new();
    let mut ordered_ids = Vec::with_capacity(model_ids.len());
    for model_id in model_ids {
        visit_model(model_id, models, &mut visit_state, &mut ordered_ids)?;
    }

    Ok(ordered_ids)
}

fn include_geometry_bounds(
    bounds: &mut Bounds,
    geometry: &Geometry,
    model_id: Option<i64>,
    models: &HashMap<i64, Model>,
    units_scale: f64,
) -> Result<(), AppError> {
    for vertex in &geometry.vertices {
        bounds.include(apply_model_chain(*vertex, model_id, models)?.scaled(units_scale));
    }

    Ok(())
}

fn build_scene_part(
    geometry: &Geometry,
    model_id: Option<i64>,
    node_index: usize,
    base_name: &str,
    materials: &HashMap<i64, ParsedMaterial>,
    connections: &SceneConnections,
    textures: &HashMap<i64, ParsedTexture>,
    videos: &HashMap<i64, ParsedVideo>,
    source_path: &Path,
    units_scale: f64,
) -> Result<ScenePart, AppError> {
    let model_material_ids = model_id
        .and_then(|id| connections.model_materials.get(&id).cloned())
        .unwrap_or_default();
    let shadings = build_shadings_for_model(
        base_name,
        &model_material_ids,
        materials,
        connections,
        textures,
        videos,
        source_path,
    )?;
    let geometry_requires_uv = shadings.iter().any(|shading| shading.diffuse_texture.is_some());

    if geometry_requires_uv && geometry.uv_layer.is_none() {
        return Err(AppError::UnsupportedFbxFeature(
            "a diffuse texture is connected, but the mesh is missing LayerElementUV data".to_owned(),
        ));
    }

    let mut part = ScenePart {
        node_index,
        resource_name: format!(
            "{}_Mesh",
            if base_name.trim().is_empty() {
                if geometry.name.trim().is_empty() {
                    "Mesh".to_owned()
                } else {
                    geometry.name.clone()
                }
            } else {
                base_name.to_owned()
            }
        ),
        positions: geometry.vertices.iter().map(|vertex| vertex.scaled(units_scale)).collect(),
        triangles: Vec::new(),
        normals: Vec::new(),
        texture_coords: Vec::new(),
        shadings,
    };

    for triangle in triangulate_polygons(&geometry.polygon_vertex_indices, geometry.vertices.len())? {
        let shading_index = resolve_material_slot(
            geometry.material_layer.as_ref(),
            triangle.polygon_index,
            part.shadings.len(),
        )?;
        let shading = &part.shadings[shading_index];
        let texture_coord_indices = if shading.diffuse_texture.is_some() {
            let uv_layer = geometry.uv_layer.as_ref().ok_or_else(|| {
                AppError::UnsupportedFbxFeature(
                    "a diffuse texture is connected, but the mesh is missing LayerElementUV data".to_owned(),
                )
            })?;
            let start_index = part.texture_coords.len();
            for (polygon_vertex_index, control_point_index) in triangle
                .polygon_vertex_indices
                .iter()
                .zip(triangle.position_indices.iter())
            {
                part.texture_coords
                    .push(uv_layer.resolve(*polygon_vertex_index, *control_point_index)?);
            }
            Some([start_index, start_index + 1, start_index + 2])
        } else {
            None
        };
        let normal_start_index = part.normals.len();
        if let Some(normal_layer) = geometry.normal_layer.as_ref() {
            for (polygon_vertex_index, control_point_index) in triangle
                .polygon_vertex_indices
                .iter()
                .zip(triangle.position_indices.iter())
            {
                part.normals
                    .push(normal_layer.resolve(*polygon_vertex_index, *control_point_index)?);
            }
        } else {
            let a = part.positions[triangle.position_indices[0]];
            let b = part.positions[triangle.position_indices[1]];
            let c = part.positions[triangle.position_indices[2]];
            let triangle_normal = (b - a).cross(c - a).normalized_or(Vec3::new(0.0, 0.0, 1.0));
            for _ in 0..3 {
                part.normals.push(triangle_normal);
            }
        }
        let normal_indices = [normal_start_index, normal_start_index + 1, normal_start_index + 2];

        part.triangles.push(SceneTriangle {
            position_indices: triangle.position_indices,
            normal_indices,
            shading_index,
            texture_coord_indices,
        });
    }

    Ok(part)
}

fn parse_geometry(node: &FbxNode) -> Result<Option<Geometry>, AppError> {
    if property_string(node.properties.get(2)) != Some("Mesh") {
        return Ok(None);
    }

    let id = property_i64(node.properties.first())
        .ok_or_else(|| AppError::UnsupportedFbxFeature("Geometry nodes must have an integer object id".to_owned()))?;

    let vertices_node = find_node(&node.children, "Vertices")
        .ok_or_else(|| AppError::UnsupportedFbxFeature("Geometry mesh is missing a Vertices array".to_owned()))?;
    let polygon_index_node = find_node(&node.children, "PolygonVertexIndex")
        .ok_or_else(|| AppError::UnsupportedFbxFeature("Geometry mesh is missing a PolygonVertexIndex array".to_owned()))?;

    let raw_vertices = property_f64_array(vertices_node.properties.first()).ok_or_else(|| {
        AppError::UnsupportedFbxFeature("Vertices must be stored as a numeric array".to_owned())
    })?;
    let polygon_vertex_indices = property_i32_array(polygon_index_node.properties.first()).ok_or_else(|| {
        AppError::UnsupportedFbxFeature("PolygonVertexIndex must be stored as an integer array".to_owned())
    })?;

    if raw_vertices.len() % 3 != 0 {
        return Err(AppError::UnsupportedFbxFeature(
            "Vertices array length is not divisible by 3".to_owned(),
        ));
    }

    let vertices = raw_vertices
        .chunks_exact(3)
        .map(|chunk| Vec3::new(chunk[0], chunk[1], chunk[2]))
        .collect();
    let uv_layer = node
        .children
        .iter()
        .find(|child| child.name == "LayerElementUV")
        .map(parse_uv_layer)
        .transpose()?;
    let normal_layer = node
        .children
        .iter()
        .find(|child| child.name == "LayerElementNormal")
        .map(parse_normal_layer)
        .transpose()?;
    let material_layer = node
        .children
        .iter()
        .find(|child| child.name == "LayerElementMaterial")
        .map(parse_material_layer)
        .transpose()?;

    Ok(Some(Geometry {
        id,
        name: clean_fbx_object_label(property_string(node.properties.get(1)).unwrap_or("Geometry")),
        vertices,
        polygon_vertex_indices,
        uv_layer,
        normal_layer,
        material_layer,
    }))
}

fn parse_model(node: &FbxNode) -> Result<Option<Model>, AppError> {
    let id = property_i64(node.properties.first())
        .ok_or_else(|| AppError::UnsupportedFbxFeature("Model nodes must have an integer object id".to_owned()))?;

    let mut model = Model {
        id,
        name: clean_fbx_object_label(property_string(node.properties.get(1)).unwrap_or("Model")),
        translation: Vec3::ZERO,
        rotation_deg: Vec3::ZERO,
        scaling: Vec3::ONE,
        rotation_order: RotationOrder::Xyz,
        parent_model: None,
    };

    if let Some(properties) = find_node(&node.children, "Properties70") {
        for property in properties.children.iter().filter(|child| child.name == "P") {
            let Some(property_name) = property_string(property.properties.first()) else {
                continue;
            };

            match property_name {
                "Lcl Translation" => {
                    model.translation = parse_property_record_vec3(property)?;
                }
                "Lcl Rotation" => {
                    model.rotation_deg = parse_property_record_vec3(property)?;
                }
                "Lcl Scaling" => {
                    model.scaling = parse_property_record_vec3(property)?;
                }
                "RotationOrder" => {
                    if property.properties.len() <= 4 {
                        return Err(AppError::UnsupportedFbxFeature(
                            "RotationOrder property is missing its numeric value".to_owned(),
                        ));
                    }
                    model.rotation_order = RotationOrder::from_fbx_value(property.properties.get(4)).ok_or_else(|| {
                        AppError::UnsupportedFbxFeature(
                            "RotationOrder property uses an unsupported value".to_owned(),
                        )
                    })?;
                }
                _ => {}
            }
        }
    }

    Ok(Some(model))
}

fn parse_material(node: &FbxNode) -> Result<Option<ParsedMaterial>, AppError> {
    let id = property_i64(node.properties.first())
        .ok_or_else(|| AppError::UnsupportedFbxFeature("Material nodes must have an integer object id".to_owned()))?;

    let mut material = ParsedMaterial {
        id,
        name: clean_fbx_object_label(property_string(node.properties.get(1)).unwrap_or("Material")),
        ambient: Vec3::new(0.18, 0.18, 0.18),
        diffuse: Vec3::new(0.705882, 0.705882, 0.705882),
        specular: Vec3::new(0.05, 0.05, 0.05),
        emissive: Vec3::ZERO,
        reflectivity: 0.0,
        opacity: 1.0,
    };

    if let Some(properties) = find_node(&node.children, "Properties70") {
        for property in properties.children.iter().filter(|child| child.name == "P") {
            let Some(property_name) = property_string(property.properties.first()) else {
                continue;
            };

            match property_name {
                "AmbientColor" | "Ambient" => material.ambient = parse_property_record_vec3(property)?,
                "DiffuseColor" | "Diffuse" => material.diffuse = parse_property_record_vec3(property)?,
                "SpecularColor" | "Specular" => material.specular = parse_property_record_vec3(property)?,
                "EmissiveColor" | "Emissive" => material.emissive = parse_property_record_vec3(property)?,
                "ReflectionFactor" | "Reflectivity" => material.reflectivity = parse_property_record_f64(property)?,
                "Opacity" => material.opacity = parse_property_record_f64(property)?,
                "TransparencyFactor" => material.opacity = 1.0 - parse_property_record_f64(property)?,
                _ => {}
            }
        }
    }

    Ok(Some(material))
}

fn parse_texture(node: &FbxNode) -> Result<Option<ParsedTexture>, AppError> {
    let id = property_i64(node.properties.first())
        .ok_or_else(|| AppError::UnsupportedFbxFeature("Texture nodes must have an integer object id".to_owned()))?;

    Ok(Some(ParsedTexture {
        id,
        name: clean_fbx_object_label(property_string(node.properties.get(1)).unwrap_or("Texture")),
        file_path_hint: read_external_file_path(node),
    }))
}

fn parse_video(node: &FbxNode) -> Result<Option<ParsedVideo>, AppError> {
    let id = property_i64(node.properties.first())
        .ok_or_else(|| AppError::UnsupportedFbxFeature("Video nodes must have an integer object id".to_owned()))?;

    Ok(Some(ParsedVideo {
        id,
        file_path_hint: read_external_file_path(node),
    }))
}

fn parse_connections(
    connections: Option<&FbxNode>,
    geometries: &HashMap<i64, Geometry>,
    models: &HashMap<i64, Model>,
    materials: &HashMap<i64, ParsedMaterial>,
    textures: &HashMap<i64, ParsedTexture>,
    videos: &HashMap<i64, ParsedVideo>,
) -> Result<SceneConnections, AppError> {
    let Some(connections) = connections else {
        return Ok(SceneConnections::default());
    };

    let mut parsed = SceneConnections::default();

    for connection in connections.children.iter().filter(|child| child.name == "C") {
        let Some(connection_kind) = property_string(connection.properties.first()) else {
            continue;
        };

        let Some(child_id) = property_i64(connection.properties.get(1)) else {
            continue;
        };
        let Some(parent_id) = property_i64(connection.properties.get(2)) else {
            continue;
        };

        match connection_kind {
            "OO" => {
                if geometries.contains_key(&child_id) && models.contains_key(&parent_id) {
                    parsed.geometry_to_models.entry(child_id).or_default().push(parent_id);
                } else if models.contains_key(&child_id) && models.contains_key(&parent_id) {
                    parsed.model_to_parent.insert(child_id, parent_id);
                } else if materials.contains_key(&child_id) && models.contains_key(&parent_id) {
                    parsed.model_materials.entry(parent_id).or_default().push(child_id);
                } else if textures.contains_key(&child_id) && materials.contains_key(&parent_id) {
                    parsed.material_textures.entry(parent_id).or_default().push(TextureBinding {
                        texture_id: child_id,
                        property_name: None,
                    });
                } else if videos.contains_key(&child_id) && textures.contains_key(&parent_id) {
                    parsed.texture_videos.insert(parent_id, child_id);
                } else if textures.contains_key(&child_id) && videos.contains_key(&parent_id) {
                    parsed.texture_videos.insert(child_id, parent_id);
                }
            }
            "OP" => {
                if textures.contains_key(&child_id) && materials.contains_key(&parent_id) {
                    parsed.material_textures.entry(parent_id).or_default().push(TextureBinding {
                        texture_id: child_id,
                        property_name: property_string(connection.properties.get(3)).map(str::to_owned),
                    });
                }
            }
            _ => {}
        }
    }

    for model_ids in parsed.geometry_to_models.values_mut() {
        model_ids.sort_unstable();
        model_ids.dedup();
    }

    Ok(parsed)
}

fn apply_model_chain(
    point: Vec3,
    model_id: Option<i64>,
    models: &HashMap<i64, Model>,
) -> Result<Vec3, AppError> {
    let Some(model_id) = model_id else {
        return Ok(point);
    };

    apply_model_chain_inner(point, model_id, models, 0)
}

fn apply_model_chain_inner(
    point: Vec3,
    model_id: i64,
    models: &HashMap<i64, Model>,
    depth: usize,
) -> Result<Vec3, AppError> {
    if depth > 128 {
        return Err(AppError::UnsupportedFbxFeature(
            "model hierarchy is too deep or cyclic".to_owned(),
        ));
    }

    let Some(model) = models.get(&model_id) else {
        return Ok(point);
    };

    let local_point = model.apply(point);

    match model.parent_model {
        Some(parent_model) => apply_model_chain_inner(local_point, parent_model, models, depth + 1),
        None => Ok(local_point),
    }
}

fn triangulate_polygons(indices: &[i32], vertex_count: usize) -> Result<Vec<TriangulatedFace>, AppError> {
    let mut triangles = Vec::new();
    let mut polygon_positions = Vec::new();
    let mut polygon_vertex_indices = Vec::new();
    let mut polygon_index = 0_usize;
    let mut polygon_vertex_cursor = 0_usize;

    for raw_index in indices {
        let is_last = *raw_index < 0;
        let resolved_index = if *raw_index < 0 {
            (-raw_index - 1) as usize
        } else {
            *raw_index as usize
        };

        if resolved_index >= vertex_count {
            return Err(AppError::UnsupportedFbxFeature(
                "PolygonVertexIndex references a vertex outside the Vertices array".to_owned(),
            ));
        }

        polygon_positions.push(resolved_index);
        polygon_vertex_indices.push(polygon_vertex_cursor);
        polygon_vertex_cursor += 1;

        if is_last {
            if polygon_positions.len() >= 3 {
                for offset in 1..polygon_positions.len() - 1 {
                    triangles.push(TriangulatedFace {
                        position_indices: [
                            polygon_positions[0],
                            polygon_positions[offset],
                            polygon_positions[offset + 1],
                        ],
                        polygon_vertex_indices: [
                            polygon_vertex_indices[0],
                            polygon_vertex_indices[offset],
                            polygon_vertex_indices[offset + 1],
                        ],
                        polygon_index,
                    });
                }
            }

            polygon_positions.clear();
            polygon_vertex_indices.clear();
            polygon_index += 1;
        }
    }

    if !polygon_positions.is_empty() {
        return Err(AppError::UnsupportedFbxFeature(
            "PolygonVertexIndex ended with an unterminated polygon".to_owned(),
        ));
    }

    Ok(triangles)
}

fn write_idtf_document(path: &Path, scene: &SceneMesh, _source_path: &Path) -> Result<(), AppError> {
    let mut scene = scene_with_notice_root(scene);
    ensure_unique_scene_resource_names(&mut scene);
    let mut document = String::new();
    let bounds = scene.bounds();
    let center = bounds.center();
    let radius = bounds.radius().max(1.0);
    let view_translation = Vec3::new(center.x, center.y - radius * 3.5, center.z + radius * 1.4);
    let light_translation = Vec3::new(center.x + radius, center.y - radius * 2.0, center.z + radius);
    let texture_count = scene
        .parts
        .iter()
        .flat_map(|part| part.shadings.iter())
        .filter(|shading| shading.diffuse_texture.is_some())
        .count();
    let shader_records = scene
        .parts
        .iter()
        .flat_map(|part| part.shadings.iter())
        .collect::<Vec<_>>();
    let texture_records = scene
        .parts
        .iter()
        .flat_map(|part| part.shadings.iter().filter_map(|shading| shading.diffuse_texture.as_ref()))
        .collect::<Vec<_>>();
    let shader_indices = shader_records
        .iter()
        .enumerate()
        .map(|(index, shading)| (shading.shader_name.clone(), index))
        .collect::<HashMap<_, _>>();

    writeln!(&mut document, "FILE_FORMAT \"IDTF\"").unwrap();
    writeln!(&mut document, "FORMAT_VERSION 100").unwrap();
    writeln!(&mut document).unwrap();

    writeln!(&mut document, "NODE \"VIEW\" {{").unwrap();
    writeln!(&mut document, "\tNODE_NAME \"DefaultView\"").unwrap();
    writeln!(&mut document, "\tPARENT_LIST {{").unwrap();
    writeln!(&mut document, "\t\tPARENT_COUNT 1").unwrap();
    writeln!(&mut document, "\t\tPARENT 0 {{").unwrap();
    writeln!(&mut document, "\t\t\tPARENT_NAME \"<NULL>\"").unwrap();
    writeln!(&mut document, "\t\t\tPARENT_TM {{").unwrap();
    writeln!(&mut document, "\t\t\t\t1.000000 0.000000 0.000000 0.000000").unwrap();
    writeln!(&mut document, "\t\t\t\t0.000000 0.258819 0.965926 0.000000").unwrap();
    writeln!(&mut document, "\t\t\t\t0.000000 -0.965926 0.258819 0.000000").unwrap();
    writeln!(
        &mut document,
        "\t\t\t\t{:.6} {:.6} {:.6} 1.000000",
        view_translation.x, view_translation.y, view_translation.z
    )
    .unwrap();
    writeln!(&mut document, "\t\t\t}}").unwrap();
    writeln!(&mut document, "\t\t}}").unwrap();
    writeln!(&mut document, "\t}}").unwrap();
    writeln!(&mut document, "\tRESOURCE_NAME \"SceneViewResource\"").unwrap();
    writeln!(&mut document, "\tVIEW_DATA {{").unwrap();
    writeln!(&mut document, "\t\tVIEW_TYPE \"PERSPECTIVE\"").unwrap();
    writeln!(&mut document, "\t\tVIEW_PROJECTION 34.515877").unwrap();
    writeln!(&mut document, "\t}}").unwrap();
    writeln!(&mut document, "}}").unwrap();
    writeln!(&mut document).unwrap();

    for node in &scene.nodes {
        writeln!(
            &mut document,
            "NODE \"{}\" {{",
            if node.mesh_index.is_some() { "MODEL" } else { "GROUP" }
        )
        .unwrap();
        writeln!(&mut document, "\tNODE_NAME \"{}\"", node.name).unwrap();
        writeln!(&mut document, "\tPARENT_LIST {{").unwrap();
        writeln!(&mut document, "\t\tPARENT_COUNT 1").unwrap();
        writeln!(&mut document, "\t\tPARENT 0 {{").unwrap();
        writeln!(
            &mut document,
            "\t\t\tPARENT_NAME \"{}\"",
            node.parent_index
                .map(|parent_index| scene.nodes[parent_index].name.as_str())
                .unwrap_or("<NULL>")
        )
        .unwrap();
        writeln!(&mut document, "\t\t\tPARENT_TM {{").unwrap();
        write_transform_matrix(&mut document, "\t\t\t\t", node.transform);
        writeln!(&mut document, "\t\t\t}}").unwrap();
        writeln!(&mut document, "\t\t}}").unwrap();
        writeln!(&mut document, "\t}}").unwrap();
        if let Some(mesh_index) = node.mesh_index {
            writeln!(
                &mut document,
                "\tRESOURCE_NAME \"{}\"",
                scene.parts[mesh_index].resource_name
            )
            .unwrap();
        }
        writeln!(&mut document, "}}").unwrap();
        writeln!(&mut document).unwrap();
    }

    writeln!(&mut document, "NODE \"LIGHT\" {{").unwrap();
    writeln!(&mut document, "\tNODE_NAME \"DefaultLight\"").unwrap();
    writeln!(&mut document, "\tPARENT_LIST {{").unwrap();
    writeln!(&mut document, "\t\tPARENT_COUNT 1").unwrap();
    writeln!(&mut document, "\t\tPARENT 0 {{").unwrap();
    writeln!(&mut document, "\t\t\tPARENT_NAME \"<NULL>\"").unwrap();
    writeln!(&mut document, "\t\t\tPARENT_TM {{").unwrap();
    writeln!(&mut document, "\t\t\t\t1.000000 0.000000 0.000000 0.000000").unwrap();
    writeln!(&mut document, "\t\t\t\t0.000000 1.000000 0.000000 0.000000").unwrap();
    writeln!(&mut document, "\t\t\t\t0.000000 0.000000 1.000000 0.000000").unwrap();
    writeln!(
        &mut document,
        "\t\t\t\t{:.6} {:.6} {:.6} 1.000000",
        light_translation.x, light_translation.y, light_translation.z
    )
    .unwrap();
    writeln!(&mut document, "\t\t\t}}").unwrap();
    writeln!(&mut document, "\t\t}}").unwrap();
    writeln!(&mut document, "\t}}").unwrap();
    writeln!(&mut document, "\tRESOURCE_NAME \"DefaultPointLight\"").unwrap();
    writeln!(&mut document, "}}").unwrap();
    writeln!(&mut document).unwrap();

    writeln!(&mut document, "RESOURCE_LIST \"VIEW\" {{").unwrap();
    writeln!(&mut document, "\tRESOURCE_COUNT 1").unwrap();
    writeln!(&mut document, "\tRESOURCE 0 {{").unwrap();
    writeln!(&mut document, "\t\tRESOURCE_NAME \"SceneViewResource\"").unwrap();
    writeln!(&mut document, "\t\tVIEW_PASS_COUNT 1").unwrap();
    writeln!(&mut document, "\t\tVIEW_ROOT_NODE_LIST {{").unwrap();
    writeln!(&mut document, "\t\t\tROOT_NODE 0 {{").unwrap();
    writeln!(&mut document, "\t\t\t\tROOT_NODE_NAME \"<NULL>\"").unwrap();
    writeln!(&mut document, "\t\t\t}}").unwrap();
    writeln!(&mut document, "\t\t}}").unwrap();
    writeln!(&mut document, "\t}}").unwrap();
    writeln!(&mut document, "}}").unwrap();
    writeln!(&mut document).unwrap();

    writeln!(&mut document, "RESOURCE_LIST \"LIGHT\" {{").unwrap();
    writeln!(&mut document, "\tRESOURCE_COUNT 1").unwrap();
    writeln!(&mut document, "\tRESOURCE 0 {{").unwrap();
    writeln!(&mut document, "\t\tRESOURCE_NAME \"DefaultPointLight\"").unwrap();
    writeln!(&mut document, "\t\tLIGHT_TYPE \"POINT\"").unwrap();
    writeln!(&mut document, "\t\tLIGHT_COLOR 1.000000 1.000000 1.000000").unwrap();
    writeln!(&mut document, "\t\tLIGHT_ATTENUATION 1.000000 0.000000 0.000000").unwrap();
    writeln!(&mut document, "\t\tLIGHT_INTENSITY 1.000000").unwrap();
    writeln!(&mut document, "\t}}").unwrap();
    writeln!(&mut document, "}}").unwrap();
    writeln!(&mut document).unwrap();

    writeln!(&mut document, "RESOURCE_LIST \"SHADER\" {{").unwrap();
    writeln!(&mut document, "\tRESOURCE_COUNT {}", shader_records.len()).unwrap();
    for (shader_index, shading) in shader_records.iter().enumerate() {
        writeln!(&mut document, "\tRESOURCE {shader_index} {{").unwrap();
        writeln!(&mut document, "\t\tRESOURCE_NAME \"{}\"", shading.shader_name).unwrap();
        writeln!(
            &mut document,
            "\t\tSHADER_MATERIAL_NAME \"{}\"",
            shading.material.material_name
        )
        .unwrap();
        writeln!(
            &mut document,
            "\t\tSHADER_ACTIVE_TEXTURE_COUNT {}",
            usize::from(shading.diffuse_texture.is_some())
        )
        .unwrap();
        if let Some(texture) = &shading.diffuse_texture {
            writeln!(&mut document, "\t\tSHADER_TEXTURE_LAYER_LIST {{").unwrap();
            writeln!(&mut document, "\t\t\tTEXTURE_LAYER 0 {{").unwrap();
            writeln!(&mut document, "\t\t\t\tTEXTURE_NAME \"{}\"", texture.texture_name).unwrap();
            writeln!(&mut document, "\t\t\t}}").unwrap();
            writeln!(&mut document, "\t\t}}").unwrap();
        }
        writeln!(&mut document, "\t}}").unwrap();
    }
    writeln!(&mut document, "}}").unwrap();
    writeln!(&mut document).unwrap();

    writeln!(&mut document, "RESOURCE_LIST \"MATERIAL\" {{").unwrap();
    writeln!(&mut document, "\tRESOURCE_COUNT {}", shader_records.len()).unwrap();
    for (material_index, shading) in shader_records.iter().enumerate() {
        writeln!(&mut document, "\tRESOURCE {material_index} {{").unwrap();
        writeln!(&mut document, "\t\tRESOURCE_NAME \"{}\"", shading.material.material_name).unwrap();
        writeln!(
            &mut document,
            "\t\tMATERIAL_AMBIENT {:.6} {:.6} {:.6}",
            shading.material.ambient.x,
            shading.material.ambient.y,
            shading.material.ambient.z
        )
        .unwrap();
        writeln!(
            &mut document,
            "\t\tMATERIAL_DIFFUSE {:.6} {:.6} {:.6}",
            shading.material.diffuse.x,
            shading.material.diffuse.y,
            shading.material.diffuse.z
        )
        .unwrap();
        writeln!(
            &mut document,
            "\t\tMATERIAL_SPECULAR {:.6} {:.6} {:.6}",
            shading.material.specular.x,
            shading.material.specular.y,
            shading.material.specular.z
        )
        .unwrap();
        writeln!(
            &mut document,
            "\t\tMATERIAL_EMISSIVE {:.6} {:.6} {:.6}",
            shading.material.emissive.x,
            shading.material.emissive.y,
            shading.material.emissive.z
        )
        .unwrap();
        writeln!(&mut document, "\t\tMATERIAL_REFLECTIVITY {:.6}", shading.material.reflectivity).unwrap();
        writeln!(&mut document, "\t\tMATERIAL_OPACITY {:.6}", shading.material.opacity).unwrap();
        writeln!(&mut document, "\t}}").unwrap();
    }
    writeln!(&mut document, "}}").unwrap();
    writeln!(&mut document).unwrap();

    if texture_count > 0 {
        writeln!(&mut document, "RESOURCE_LIST \"TEXTURE\" {{").unwrap();
        writeln!(&mut document, "\tRESOURCE_COUNT {texture_count}").unwrap();
        for (texture_index, texture) in texture_records.iter().enumerate() {
            writeln!(&mut document, "\tRESOURCE {texture_index} {{").unwrap();
            writeln!(&mut document, "\t\tRESOURCE_NAME \"{}\"", texture.texture_name).unwrap();
            writeln!(
                &mut document,
                "\t\tTEXTURE_PATH \"{}\"",
                texture.idtf_path.replace('\\', "/")
            )
            .unwrap();
            writeln!(&mut document, "\t}}").unwrap();
        }
        writeln!(&mut document, "}}").unwrap();
        writeln!(&mut document).unwrap();
    }

    writeln!(&mut document, "RESOURCE_LIST \"MODEL\" {{").unwrap();
    writeln!(&mut document, "\tRESOURCE_COUNT {}", scene.parts.len()).unwrap();
    for (mesh_index, part) in scene.parts.iter().enumerate() {
        writeln!(&mut document, "\tRESOURCE {mesh_index} {{").unwrap();
        writeln!(&mut document, "\t\tRESOURCE_NAME \"{}\"", part.resource_name).unwrap();
        writeln!(&mut document, "\t\tMODEL_TYPE \"MESH\"").unwrap();
        writeln!(&mut document, "\t\tMESH {{").unwrap();
        writeln!(&mut document, "\t\t\tFACE_COUNT {}", part.triangles.len()).unwrap();
        writeln!(&mut document, "\t\t\tMODEL_POSITION_COUNT {}", part.positions.len()).unwrap();
        writeln!(&mut document, "\t\t\tMODEL_NORMAL_COUNT {}", part.normals.len()).unwrap();
        writeln!(&mut document, "\t\t\tMODEL_DIFFUSE_COLOR_COUNT 0").unwrap();
        writeln!(&mut document, "\t\t\tMODEL_SPECULAR_COLOR_COUNT 0").unwrap();
        writeln!(&mut document, "\t\t\tMODEL_TEXTURE_COORD_COUNT {}", part.texture_coords.len()).unwrap();
        writeln!(&mut document, "\t\t\tMODEL_BONE_COUNT 0").unwrap();
        writeln!(&mut document, "\t\t\tMODEL_SHADING_COUNT {}", part.shadings.len()).unwrap();
        writeln!(&mut document, "\t\t\tMODEL_SHADING_DESCRIPTION_LIST {{").unwrap();
        for (shading_index, shading) in part.shadings.iter().enumerate() {
            writeln!(&mut document, "\t\t\t\tSHADING_DESCRIPTION {shading_index} {{").unwrap();
            writeln!(
                &mut document,
                "\t\t\t\t\tTEXTURE_LAYER_COUNT {}",
                usize::from(shading.diffuse_texture.is_some())
            )
            .unwrap();
            if shading.diffuse_texture.is_some() {
                writeln!(&mut document, "\t\t\t\t\tTEXTURE_COORD_DIMENSION_LIST {{").unwrap();
                writeln!(&mut document, "\t\t\t\t\t\tTEXTURE_LAYER 0 DIMENSION: 2").unwrap();
                writeln!(&mut document, "\t\t\t\t\t}}").unwrap();
            }
            writeln!(
                &mut document,
                "\t\t\t\t\tSHADER_ID {}",
                shader_indices[&shading.shader_name]
            )
            .unwrap();
            writeln!(&mut document, "\t\t\t\t}}").unwrap();
        }
        writeln!(&mut document, "\t\t\t}}").unwrap();
        writeln!(&mut document, "\t\t\tMESH_FACE_POSITION_LIST {{").unwrap();
        for triangle in &part.triangles {
            writeln!(
                &mut document,
                "\t\t\t\t{} {} {}",
                triangle.position_indices[0],
                triangle.position_indices[1],
                triangle.position_indices[2]
            )
            .unwrap();
        }
        writeln!(&mut document, "\t\t\t}}").unwrap();
        writeln!(&mut document, "\t\t\tMESH_FACE_NORMAL_LIST {{").unwrap();
        for triangle in &part.triangles {
            writeln!(
                &mut document,
                "\t\t\t\t{} {} {}",
                triangle.normal_indices[0],
                triangle.normal_indices[1],
                triangle.normal_indices[2]
            )
            .unwrap();
        }
        writeln!(&mut document, "\t\t\t}}").unwrap();
        writeln!(&mut document, "\t\t\tMESH_FACE_SHADING_LIST {{").unwrap();
        for triangle in &part.triangles {
            writeln!(&mut document, "\t\t\t\t{}", triangle.shading_index).unwrap();
        }
        writeln!(&mut document, "\t\t\t}}").unwrap();
        if !part.texture_coords.is_empty() {
            writeln!(&mut document, "\t\t\tMESH_FACE_TEXTURE_COORD_LIST {{").unwrap();
            for (face_index, triangle) in part.triangles.iter().enumerate() {
                if let Some(texture_coord_indices) = triangle.texture_coord_indices {
                    writeln!(&mut document, "\t\t\t\tFACE {face_index} {{").unwrap();
                    writeln!(
                        &mut document,
                        "\t\t\t\t\tTEXTURE_LAYER 0 TEX_COORD: {} {} {}",
                        texture_coord_indices[0],
                        texture_coord_indices[1],
                        texture_coord_indices[2]
                    )
                    .unwrap();
                    writeln!(&mut document, "\t\t\t\t}}").unwrap();
                }
            }
            writeln!(&mut document, "\t\t\t}}").unwrap();
        }
        writeln!(&mut document, "\t\t\tMODEL_POSITION_LIST {{").unwrap();
        for position in &part.positions {
            writeln!(&mut document, "\t\t\t\t{:.6} {:.6} {:.6}", position.x, position.y, position.z).unwrap();
        }
        writeln!(&mut document, "\t\t\t}}").unwrap();
        writeln!(&mut document, "\t\t\tMODEL_NORMAL_LIST {{").unwrap();
        for normal in &part.normals {
            writeln!(&mut document, "\t\t\t\t{:.6} {:.6} {:.6}", normal.x, normal.y, normal.z).unwrap();
        }
        writeln!(&mut document, "\t\t\t}}").unwrap();
        if !part.texture_coords.is_empty() {
            writeln!(&mut document, "\t\t\tMODEL_TEXTURE_COORD_LIST {{").unwrap();
            for texture_coord in &part.texture_coords {
                writeln!(
                    &mut document,
                    "\t\t\t\t{:.6} {:.6} 0.000000 0.000000",
                    texture_coord.x,
                    texture_coord.y
                )
                .unwrap();
            }
            writeln!(&mut document, "\t\t\t}}").unwrap();
        }
        writeln!(&mut document, "\t\t}}").unwrap();
        writeln!(&mut document, "\t}}").unwrap();
    }
    writeln!(&mut document, "}}").unwrap();
    writeln!(&mut document).unwrap();

    for part in &scene.parts {
        writeln!(&mut document, "MODIFIER \"SHADING\" {{").unwrap();
        writeln!(
            &mut document,
            "\tMODIFIER_NAME \"{}\"",
            scene.nodes[part.node_index].name
        )
        .unwrap();
        writeln!(&mut document, "\tPARAMETERS {{").unwrap();
        writeln!(&mut document, "\t\tSHADER_LIST_COUNT {}", part.shadings.len()).unwrap();
        writeln!(&mut document, "\t\tSHADER_LIST_LIST {{").unwrap();
        for (shading_index, shading) in part.shadings.iter().enumerate() {
            writeln!(&mut document, "\t\t\tSHADER_LIST {shading_index} {{").unwrap();
            writeln!(&mut document, "\t\t\t\tSHADER_COUNT 1").unwrap();
            writeln!(&mut document, "\t\t\t\tSHADER_NAME_LIST {{").unwrap();
            writeln!(
                &mut document,
                "\t\t\t\t\tSHADER 0 NAME: \"{}\"",
                shading.shader_name
            )
            .unwrap();
            writeln!(&mut document, "\t\t\t\t}}").unwrap();
            writeln!(&mut document, "\t\t\t}}").unwrap();
        }
        writeln!(&mut document, "\t\t}}").unwrap();
        writeln!(&mut document, "\t}}").unwrap();
        writeln!(&mut document, "}}").unwrap();
        writeln!(&mut document).unwrap();
    }

    let mut file = File::create(path)?;
    file.write_all(document.as_bytes())?;
    Ok(())
}

fn scene_with_notice_root(scene: &SceneMesh) -> SceneMesh {
    let mut scene = scene.clone();
    let notice_mesh_index = scene.parts.len();

    for node in &mut scene.nodes {
        node.parent_index = Some(node.parent_index.map_or(0, |parent_index| parent_index + 1));
    }

    for part in &mut scene.parts {
        part.node_index += 1;
    }

    scene.nodes.insert(
        0,
        SceneNode {
            name: FBX2U3D_NOTICE_NODE_NAME.to_owned(),
            parent_index: None,
            transform: TransformMatrix::identity(),
            mesh_index: Some(notice_mesh_index),
        },
    );

    scene.parts.push(notice_scene_part(scene.bounds));

    scene
}

fn notice_scene_part(bounds: Bounds) -> ScenePart {
    let has_finite_bounds = bounds.min.x.is_finite()
        && bounds.min.y.is_finite()
        && bounds.min.z.is_finite()
        && bounds.max.x.is_finite()
        && bounds.max.y.is_finite()
        && bounds.max.z.is_finite();
    let center = if has_finite_bounds { bounds.center() } else { Vec3::ZERO };
    let size = if has_finite_bounds {
        bounds.max - bounds.min
    } else {
        Vec3::ONE
    };
    let x_extent = if size.x.abs() > f64::EPSILON {
        size.x.abs() * 0.05
    } else {
        bounds.radius() * 0.01
    };
    let y_extent = if size.y.abs() > f64::EPSILON {
        size.y.abs() * 0.05
    } else {
        bounds.radius() * 0.01
    };

    ScenePart {
        node_index: 0,
        resource_name: FBX2U3D_NOTICE_MESH_NAME.to_owned(),
        positions: vec![
            center,
            center + Vec3::new(x_extent.max(0.01), 0.0, 0.0),
            center + Vec3::new(0.0, y_extent.max(0.01), 0.0),
        ],
        triangles: vec![SceneTriangle {
            position_indices: [0, 1, 2],
            normal_indices: [0, 1, 2],
            shading_index: 0,
            texture_coord_indices: None,
        }],
        normals: vec![Vec3::new(0.0, 0.0, 1.0); 3],
        texture_coords: Vec::new(),
        shadings: vec![SceneShading {
            shader_name: FBX2U3D_NOTICE_SHADER_NAME.to_owned(),
            material: SceneMaterial {
                material_name: FBX2U3D_NOTICE_MATERIAL_NAME.to_owned(),
                ambient: Vec3::ZERO,
                diffuse: Vec3::ZERO,
                specular: Vec3::ZERO,
                emissive: Vec3::ZERO,
                reflectivity: 0.0,
                opacity: 0.0,
            },
            diffuse_texture: None,
        }],
    }
}

fn write_transform_matrix(document: &mut String, indent: &str, transform: TransformMatrix) {
    for row in transform.rows {
        writeln!(
            document,
            "{indent}{:.6} {:.6} {:.6} {:.6}",
            row[0], row[1], row[2], row[3]
        )
        .unwrap();
    }
}

fn run_idtf_converter(converter: &Path, input: &Path, output: &Path) -> Result<(), AppError> {
    let result = Command::new(converter)
        .current_dir(input.parent().unwrap_or_else(|| Path::new(".")))
        .arg("-input")
        .arg(input)
        .arg("-output")
        .arg(output)
        .arg("-pq")
        .arg(U3D_MAX_RESOURCE_QUALITY)
        .arg("-tcq")
        .arg(U3D_MAX_RESOURCE_QUALITY)
        .arg("-nq")
        .arg(U3D_MAX_RESOURCE_QUALITY)
        .arg("-gq")
        .arg(U3D_MAX_RESOURCE_QUALITY)
        .output()?;

    if result.status.success() && output.is_file() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    Err(AppError::ConversionFailed(format!(
        "IDTFConverter.exe exited with {:?}. stdout: {} stderr: {}",
        result.status.code(),
        stdout.trim(),
        stderr.trim()
    )))
}

fn find_node<'a>(nodes: &'a [FbxNode], name: &str) -> Option<&'a FbxNode> {
    nodes.iter().find(|node| node.name == name)
}

fn property_string(property: Option<&FbxProperty>) -> Option<&str> {
    match property? {
        FbxProperty::String(value) => Some(value.as_str()),
        _ => None,
    }
}

fn property_i64(property: Option<&FbxProperty>) -> Option<i64> {
    match property? {
        FbxProperty::I64(value) => Some(*value),
        FbxProperty::I32(value) => Some(i64::from(*value)),
        _ => None,
    }
}

fn property_i32(property: Option<&FbxProperty>) -> Option<i32> {
    match property? {
        FbxProperty::I32(value) => Some(*value),
        FbxProperty::I64(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn property_f64(property: Option<&FbxProperty>) -> Option<f64> {
    match property? {
        FbxProperty::F64(value) => Some(*value),
        FbxProperty::F32(value) => Some(f64::from(*value)),
        FbxProperty::I32(value) => Some(f64::from(*value)),
        FbxProperty::I64(value) => Some(*value as f64),
        _ => None,
    }
}

fn property_f64_array(property: Option<&FbxProperty>) -> Option<Vec<f64>> {
    match property? {
        FbxProperty::F64Array(values) => Some(values.clone()),
        FbxProperty::F32Array(values) => Some(values.iter().map(|value| f64::from(*value)).collect()),
        _ => None,
    }
}

fn property_i32_array(property: Option<&FbxProperty>) -> Option<Vec<i32>> {
    match property? {
        FbxProperty::I32Array(values) => Some(values.clone()),
        FbxProperty::I64Array(values) => values.iter().copied().map(i32::try_from).collect::<Result<Vec<_>, _>>().ok(),
        _ => None,
    }
}

fn parse_property_vec3(properties: &[FbxProperty]) -> Result<Vec3, AppError> {
    if properties.len() < 3 {
        return Err(AppError::UnsupportedFbxFeature(
            "expected three numeric values in an FBX Properties70 record".to_owned(),
        ));
    }

    let x = property_f64(properties.first()).ok_or_else(|| {
        AppError::UnsupportedFbxFeature("failed to read X component from Properties70".to_owned())
    })?;
    let y = property_f64(properties.get(1)).ok_or_else(|| {
        AppError::UnsupportedFbxFeature("failed to read Y component from Properties70".to_owned())
    })?;
    let z = property_f64(properties.get(2)).ok_or_else(|| {
        AppError::UnsupportedFbxFeature("failed to read Z component from Properties70".to_owned())
    })?;

    Ok(Vec3::new(x, y, z))
}

fn parse_property_record_vec3(property: &FbxNode) -> Result<Vec3, AppError> {
    if property.properties.len() <= 6 {
        return Err(AppError::UnsupportedFbxFeature(
            "expected at least seven fields in an FBX Properties70 record".to_owned(),
        ));
    }

    parse_property_vec3(&property.properties[4..7])
}

fn parse_property_record_f64(property: &FbxNode) -> Result<f64, AppError> {
    if property.properties.len() <= 4 {
        return Err(AppError::UnsupportedFbxFeature(
            "expected at least five fields in an FBX Properties70 record".to_owned(),
        ));
    }

    property_f64(property.properties.get(4)).ok_or_else(|| {
        AppError::UnsupportedFbxFeature("failed to read numeric value from Properties70".to_owned())
    })
}

fn parse_uv_layer(node: &FbxNode) -> Result<UvLayer, AppError> {
    let mapping = LayerMapping::from_fbx_value(
        node_child_string(node, "MappingInformationType")
            .ok_or_else(|| AppError::UnsupportedFbxFeature("LayerElementUV is missing MappingInformationType".to_owned()))?,
    )?;
    let reference = ReferenceType::from_fbx_value(
        node_child_string(node, "ReferenceInformationType").ok_or_else(|| {
            AppError::UnsupportedFbxFeature("LayerElementUV is missing ReferenceInformationType".to_owned())
        })?,
    )?;
    let raw_uvs = property_f64_array(
        find_node(&node.children, "UV").and_then(|child| child.properties.first()),
    )
    .ok_or_else(|| AppError::UnsupportedFbxFeature("LayerElementUV is missing the UV array".to_owned()))?;

    if raw_uvs.len() % 2 != 0 {
        return Err(AppError::UnsupportedFbxFeature(
            "LayerElementUV UV array length is not divisible by 2".to_owned(),
        ));
    }

    Ok(UvLayer {
        mapping,
        reference,
        values: raw_uvs
            .chunks_exact(2)
            .map(|chunk| Vec2::new(chunk[0], chunk[1]))
            .collect(),
        indices: property_i32_array(find_node(&node.children, "UVIndex").and_then(|child| child.properties.first()))
            .unwrap_or_default(),
    })
}

fn parse_normal_layer(node: &FbxNode) -> Result<NormalLayer, AppError> {
    let mapping = LayerMapping::from_fbx_value(
        node_child_string(node, "MappingInformationType").ok_or_else(|| {
            AppError::UnsupportedFbxFeature("LayerElementNormal is missing MappingInformationType".to_owned())
        })?,
    )?;
    let reference = ReferenceType::from_fbx_value(
        node_child_string(node, "ReferenceInformationType").ok_or_else(|| {
            AppError::UnsupportedFbxFeature("LayerElementNormal is missing ReferenceInformationType".to_owned())
        })?,
    )?;
    let raw_normals = property_f64_array(
        find_node(&node.children, "Normals").and_then(|child| child.properties.first()),
    )
    .ok_or_else(|| AppError::UnsupportedFbxFeature("LayerElementNormal is missing the Normals array".to_owned()))?;

    if raw_normals.len() % 3 != 0 {
        return Err(AppError::UnsupportedFbxFeature(
            "LayerElementNormal Normals array length is not divisible by 3".to_owned(),
        ));
    }

    Ok(NormalLayer {
        mapping,
        reference,
        values: raw_normals
            .chunks_exact(3)
            .map(|chunk| Vec3::new(chunk[0], chunk[1], chunk[2]).normalized_or(Vec3::new(0.0, 0.0, 1.0)))
            .collect(),
        indices: property_i32_array(
            find_node(&node.children, "NormalsIndex").and_then(|child| child.properties.first()),
        )
        .unwrap_or_default(),
    })
}

fn parse_material_layer(node: &FbxNode) -> Result<MaterialLayer, AppError> {
    let mapping = MaterialMapping::from_fbx_value(
        node_child_string(node, "MappingInformationType").ok_or_else(|| {
            AppError::UnsupportedFbxFeature("LayerElementMaterial is missing MappingInformationType".to_owned())
        })?,
    )?;
    let materials = property_i32_array(
        find_node(&node.children, "Materials").and_then(|child| child.properties.first()),
    )
    .unwrap_or_default();

    Ok(MaterialLayer { mapping, materials })
}

fn resolve_material_slot(
    material_layer: Option<&MaterialLayer>,
    polygon_index: usize,
    shading_count: usize,
) -> Result<usize, AppError> {
    let slot = material_layer
        .and_then(|layer| layer.slot_for_polygon(polygon_index))
        .unwrap_or(0);

    if slot >= shading_count {
        return Err(AppError::UnsupportedFbxFeature(format!(
            "material layer references slot {slot}, but only {shading_count} material bindings were found"
        )));
    }

    Ok(slot)
}

fn build_shadings_for_model(
    model_name: &str,
    model_material_ids: &[i64],
    materials: &HashMap<i64, ParsedMaterial>,
    connections: &SceneConnections,
    textures: &HashMap<i64, ParsedTexture>,
    videos: &HashMap<i64, ParsedVideo>,
    source_path: &Path,
) -> Result<Vec<SceneShading>, AppError> {
    let base_name = sanitize_idtf_name(model_name);

    if model_material_ids.is_empty() {
        return Ok(vec![SceneShading {
            shader_name: format!("{base_name}_Shader1"),
            material: SceneMaterial {
                material_name: format!("{base_name}_Material1"),
                ambient: Vec3::new(0.18, 0.18, 0.18),
                diffuse: Vec3::new(0.705882, 0.705882, 0.705882),
                specular: Vec3::new(0.05, 0.05, 0.05),
                emissive: Vec3::ZERO,
                reflectivity: 0.0,
                opacity: 1.0,
            },
            diffuse_texture: None,
        }]);
    }

    model_material_ids
        .iter()
        .enumerate()
        .map(|(slot, material_id)| {
            let parsed_material = materials.get(material_id);
            let material_name = parsed_material
                .map(|material| sanitize_idtf_name(&format!("{base_name}_{}", material.name)))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("{base_name}_Material{}", slot + 1));
            let diffuse_texture = resolve_material_texture(
                *material_id,
                &base_name,
                slot,
                connections,
                textures,
                videos,
                source_path,
            )?;

            Ok(SceneShading {
                shader_name: format!("{base_name}_Shader{}", slot + 1),
                material: SceneMaterial {
                    material_name,
                    ambient: parsed_material.map(|material| material.ambient).unwrap_or(Vec3::new(0.18, 0.18, 0.18)),
                    diffuse: parsed_material
                        .map(|material| material.diffuse)
                        .unwrap_or(Vec3::new(0.705882, 0.705882, 0.705882)),
                    specular: parsed_material.map(|material| material.specular).unwrap_or(Vec3::new(0.05, 0.05, 0.05)),
                    emissive: parsed_material.map(|material| material.emissive).unwrap_or(Vec3::ZERO),
                    reflectivity: parsed_material.map(|material| material.reflectivity).unwrap_or(0.0),
                    opacity: parsed_material.map(|material| material.opacity).unwrap_or(1.0),
                },
                diffuse_texture,
            })
        })
        .collect()
}

fn resolve_material_texture(
    material_id: i64,
    model_name: &str,
    slot: usize,
    connections: &SceneConnections,
    textures: &HashMap<i64, ParsedTexture>,
    videos: &HashMap<i64, ParsedVideo>,
    source_path: &Path,
) -> Result<Option<SceneTexture>, AppError> {
    let Some(bindings) = connections.material_textures.get(&material_id) else {
        return Ok(None);
    };

    let binding = bindings
        .iter()
        .find(|binding| {
            binding
                .property_name
                .as_deref()
                .is_none_or(|property_name| property_name.eq_ignore_ascii_case("DiffuseColor") || property_name.to_ascii_lowercase().contains("diffuse"))
        })
        .or_else(|| bindings.first())
        .ok_or_else(|| AppError::UnsupportedFbxFeature("material texture bindings were present but empty".to_owned()))?;

    let parsed_texture = textures.get(&binding.texture_id).ok_or_else(|| {
        AppError::UnsupportedFbxFeature(format!(
            "texture connection references unknown texture object id {}",
            binding.texture_id
        ))
    })?;
    let resolved_path = resolve_texture_source_path(binding.texture_id, connections, textures, videos, source_path)?;
    let default_file_name = format!("texture-{}", slot + 1);
    let staged_name = resolved_path
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .map(str::to_owned)
        .unwrap_or(default_file_name);

    Ok(Some(SceneTexture {
        texture_name: sanitize_idtf_name(&format!("{model_name}_{}_{}", parsed_texture.name, slot + 1)),
        source_path: resolved_path,
        idtf_path: staged_name,
    }))
}

fn resolve_texture_source_path(
    texture_id: i64,
    connections: &SceneConnections,
    textures: &HashMap<i64, ParsedTexture>,
    videos: &HashMap<i64, ParsedVideo>,
    source_path: &Path,
) -> Result<PathBuf, AppError> {
    let texture = textures.get(&texture_id).ok_or_else(|| {
        AppError::UnsupportedFbxFeature(format!("texture connection references unknown texture object id {texture_id}"))
    })?;
    let raw_path = texture
        .file_path_hint
        .as_ref()
        .or_else(|| {
            connections
                .texture_videos
                .get(&texture_id)
                .and_then(|video_id| videos.get(video_id))
                .and_then(|video| video.file_path_hint.as_ref())
        })
        .ok_or_else(|| {
            AppError::UnsupportedFbxFeature(
                "a diffuse texture is connected, but no FileName or RelativeFilename was found".to_owned(),
            )
        })?;
    let resolved_path = resolve_external_asset_path(raw_path, source_path);

    if !resolved_path.is_file() {
        return Err(AppError::UnsupportedFbxFeature(format!(
            "diffuse texture file could not be found: {}",
            resolved_path.display()
        )));
    }

    Ok(resolved_path)
}

fn resolve_external_asset_path(raw_path: &str, source_path: &Path) -> PathBuf {
    let candidate = PathBuf::from(raw_path);
    if candidate.is_absolute() {
        candidate
    } else {
        source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(candidate)
    }
}

fn stage_scene_assets(scene: &SceneMesh, staging_dir: &Path) -> Result<SceneMesh, AppError> {
    let mut staged_scene = scene.clone();
    let mut used_file_names = HashMap::<String, usize>::new();

    for part in &mut staged_scene.parts {
        for shading in &mut part.shadings {
            if let Some(texture) = &mut shading.diffuse_texture {
                let base_name = sanitize_file_component(
                    texture
                        .source_path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or(&texture.texture_name),
                );
                let extension = texture
                    .source_path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(|extension| format!(".{extension}"))
                    .unwrap_or_default();
                let sequence = used_file_names.entry(base_name.clone()).or_insert(0);
                let mut local_sequence = *sequence;
                let staged_name = loop {
                    let candidate = if local_sequence == 0 {
                        format!("{base_name}{extension}")
                    } else {
                        format!("{base_name}-{}{extension}", local_sequence)
                    };
                    if staging_dir.join(&candidate) != texture.source_path {
                        break candidate;
                    }
                    local_sequence += 1;
                };
                *sequence = local_sequence + 1;

                fs::copy(&texture.source_path, staging_dir.join(&staged_name))?;
                texture.idtf_path = staged_name;
            }
        }
    }

    Ok(staged_scene)
}

fn ensure_unique_scene_resource_names(scene: &mut SceneMesh) {
    let mut node_names = HashSet::new();
    for (index, node) in scene.nodes.iter_mut().enumerate() {
        node.name = unique_resource_name(&node.name, &format!("Node{}", index + 1), &mut node_names);
    }

    let mut mesh_resource_names = HashSet::new();
    for (index, part) in scene.parts.iter_mut().enumerate() {
        part.resource_name = unique_resource_name(
            &part.resource_name,
            &format!("Mesh{}", index + 1),
            &mut mesh_resource_names,
        );
    }

    let mut shader_names = HashSet::new();
    let mut material_names = HashSet::new();
    let mut texture_names = HashSet::new();

    let mut shading_index = 0_usize;
    for part in &mut scene.parts {
        for shading in &mut part.shadings {
            shading.shader_name = unique_resource_name(
                &shading.shader_name,
                &format!("Shader{}", shading_index + 1),
                &mut shader_names,
            );
            shading.material.material_name = unique_resource_name(
                &shading.material.material_name,
                &format!("Material{}", shading_index + 1),
                &mut material_names,
            );

            if let Some(texture) = &mut shading.diffuse_texture {
                texture.texture_name = unique_resource_name(
                    &texture.texture_name,
                    &format!("Texture{}", shading_index + 1),
                    &mut texture_names,
                );
            }

            shading_index += 1;
        }
    }
}

fn unique_resource_name(name: &str, fallback: &str, used_names: &mut HashSet<String>) -> String {
    let base_name = sanitize_idtf_name(name);
    let base_name = if base_name.trim().is_empty() {
        fallback.to_owned()
    } else {
        base_name
    };

    let mut candidate = base_name.clone();
    let mut suffix = 2_usize;
    while used_names.contains(&candidate) {
        candidate = format!("{base_name}_{suffix}");
        suffix += 1;
    }
    used_names.insert(candidate.clone());
    candidate
}

fn read_external_file_path(node: &FbxNode) -> Option<String> {
    for node_name in ["RelativeFilename", "RelativeFileName", "FileName", "Filename"] {
        if let Some(path) = node_child_string(node, node_name) {
            return Some(path.to_owned());
        }
    }

    let properties = find_node(&node.children, "Properties70")?;
    for property in properties.children.iter().filter(|child| child.name == "P") {
        let Some(property_name) = property_string(property.properties.first()) else {
            continue;
        };
        if matches!(property_name, "RelativeFilename" | "RelativeFileName" | "FileName" | "Filename") {
            if let Some(value) = property_string(property.properties.get(4)) {
                return Some(value.to_owned());
            }
        }
    }

    None
}

fn node_child_string<'a>(node: &'a FbxNode, child_name: &str) -> Option<&'a str> {
    property_string(find_node(&node.children, child_name).and_then(|child| child.properties.first()))
}

fn sanitize_idtf_name(name: &str) -> String {
    name.chars()
        .map(|character| match character {
            '"' | '\n' | '\r' | '\t' => '_',
            other => other,
        })
        .collect()
}

fn sanitize_file_component(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            other => other,
        })
        .collect();

    if sanitized.is_empty() {
        "texture".to_owned()
    } else {
        sanitized
    }
}

fn clean_fbx_object_label(raw: &str) -> String {
    raw.split('\0')
        .next()
        .unwrap_or(raw)
        .rsplit("::")
        .next()
        .unwrap_or(raw)
        .to_owned()
}

#[derive(Debug, Clone)]
struct Geometry {
    id: i64,
    name: String,
    vertices: Vec<Vec3>,
    polygon_vertex_indices: Vec<i32>,
    uv_layer: Option<UvLayer>,
    normal_layer: Option<NormalLayer>,
    material_layer: Option<MaterialLayer>,
}

#[derive(Debug, Clone)]
struct Model {
    id: i64,
    name: String,
    translation: Vec3,
    rotation_deg: Vec3,
    scaling: Vec3,
    rotation_order: RotationOrder,
    parent_model: Option<i64>,
}

impl Model {
    fn apply(&self, point: Vec3) -> Vec3 {
        let scaled = Vec3::new(
            point.x * self.scaling.x,
            point.y * self.scaling.y,
            point.z * self.scaling.z,
        );
        let rotated = self.rotation_order.apply(scaled, self.rotation_deg);
        rotated + self.translation
    }

    fn transform_matrix(&self, units_scale: f64) -> TransformMatrix {
        let origin = self.apply(Vec3::ZERO);
        let x_axis = self.apply(Vec3::new(1.0, 0.0, 0.0)) - origin;
        let y_axis = self.apply(Vec3::new(0.0, 1.0, 0.0)) - origin;
        let z_axis = self.apply(Vec3::new(0.0, 0.0, 1.0)) - origin;

        TransformMatrix {
            rows: [
                [x_axis.x, x_axis.y, x_axis.z, 0.0],
                [y_axis.x, y_axis.y, y_axis.z, 0.0],
                [z_axis.x, z_axis.y, z_axis.z, 0.0],
                [origin.x * units_scale, origin.y * units_scale, origin.z * units_scale, 1.0],
            ],
        }
    }
}

#[derive(Debug, Clone)]
struct ParsedMaterial {
    id: i64,
    name: String,
    ambient: Vec3,
    diffuse: Vec3,
    specular: Vec3,
    emissive: Vec3,
    reflectivity: f64,
    opacity: f64,
}

#[derive(Debug, Clone)]
struct ParsedTexture {
    id: i64,
    name: String,
    file_path_hint: Option<String>,
}

#[derive(Debug, Clone)]
struct ParsedVideo {
    id: i64,
    file_path_hint: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct SceneConnections {
    geometry_to_models: HashMap<i64, Vec<i64>>,
    model_to_parent: HashMap<i64, i64>,
    model_materials: HashMap<i64, Vec<i64>>,
    material_textures: HashMap<i64, Vec<TextureBinding>>,
    texture_videos: HashMap<i64, i64>,
}

#[derive(Debug, Clone)]
struct TextureBinding {
    texture_id: i64,
    property_name: Option<String>,
}

#[derive(Debug, Clone)]
struct UvLayer {
    mapping: LayerMapping,
    reference: ReferenceType,
    values: Vec<Vec2>,
    indices: Vec<i32>,
}

impl UvLayer {
    fn resolve(&self, polygon_vertex_index: usize, control_point_index: usize) -> Result<Vec2, AppError> {
        let logical_index = match self.mapping {
            LayerMapping::ByPolygonVertex => polygon_vertex_index,
            LayerMapping::ByControlPoint => control_point_index,
        };
        let direct_index = match self.reference {
            ReferenceType::Direct => logical_index,
            ReferenceType::IndexToDirect => self
                .indices
                .get(logical_index)
                .and_then(|index| usize::try_from(*index).ok())
                .ok_or_else(|| {
                    AppError::UnsupportedFbxFeature(
                        "LayerElementUV IndexToDirect mapping referenced a missing UV index".to_owned(),
                    )
                })?,
        };

        self.values.get(direct_index).copied().ok_or_else(|| {
            AppError::UnsupportedFbxFeature(
                "LayerElementUV referenced a UV coordinate outside the UV array".to_owned(),
            )
        })
    }
}

#[derive(Debug, Clone)]
struct NormalLayer {
    mapping: LayerMapping,
    reference: ReferenceType,
    values: Vec<Vec3>,
    indices: Vec<i32>,
}

impl NormalLayer {
    fn resolve(&self, polygon_vertex_index: usize, control_point_index: usize) -> Result<Vec3, AppError> {
        let logical_index = match self.mapping {
            LayerMapping::ByPolygonVertex => polygon_vertex_index,
            LayerMapping::ByControlPoint => control_point_index,
        };
        let direct_index = match self.reference {
            ReferenceType::Direct => logical_index,
            ReferenceType::IndexToDirect => self
                .indices
                .get(logical_index)
                .and_then(|index| usize::try_from(*index).ok())
                .ok_or_else(|| {
                    AppError::UnsupportedFbxFeature(
                        "LayerElementNormal IndexToDirect mapping referenced a missing normal index".to_owned(),
                    )
                })?,
        };

        self.values.get(direct_index).copied().ok_or_else(|| {
            AppError::UnsupportedFbxFeature(
                "LayerElementNormal referenced a normal outside the normal array".to_owned(),
            )
        })
    }
}

#[derive(Debug, Clone)]
struct MaterialLayer {
    mapping: MaterialMapping,
    materials: Vec<i32>,
}

impl MaterialLayer {
    fn slot_for_polygon(&self, polygon_index: usize) -> Option<usize> {
        let raw_slot = match self.mapping {
            MaterialMapping::AllSame => *self.materials.first().unwrap_or(&0),
            MaterialMapping::ByPolygon => *self.materials.get(polygon_index).or_else(|| self.materials.first()).unwrap_or(&0),
        };

        usize::try_from(raw_slot).ok()
    }
}

#[derive(Debug, Clone, Copy)]
enum LayerMapping {
    ByPolygonVertex,
    ByControlPoint,
}

impl LayerMapping {
    fn from_fbx_value(value: &str) -> Result<Self, AppError> {
        match value {
            "ByPolygonVertex" => Ok(Self::ByPolygonVertex),
            "ByVertice" | "ByVertex" | "ByControlPoint" => Ok(Self::ByControlPoint),
            other => Err(AppError::UnsupportedFbxFeature(format!(
                "unsupported layer mapping type: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum MaterialMapping {
    AllSame,
    ByPolygon,
}

impl MaterialMapping {
    fn from_fbx_value(value: &str) -> Result<Self, AppError> {
        match value {
            "AllSame" => Ok(Self::AllSame),
            "ByPolygon" => Ok(Self::ByPolygon),
            other => Err(AppError::UnsupportedFbxFeature(format!(
                "unsupported material mapping type: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ReferenceType {
    Direct,
    IndexToDirect,
}

impl ReferenceType {
    fn from_fbx_value(value: &str) -> Result<Self, AppError> {
        match value {
            "Direct" => Ok(Self::Direct),
            "IndexToDirect" => Ok(Self::IndexToDirect),
            other => Err(AppError::UnsupportedFbxFeature(format!(
                "unsupported layer reference type: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
struct TriangulatedFace {
    position_indices: [usize; 3],
    polygon_vertex_indices: [usize; 3],
    polygon_index: usize,
}

#[derive(Debug, Clone)]
struct SceneTriangle {
    position_indices: [usize; 3],
    normal_indices: [usize; 3],
    shading_index: usize,
    texture_coord_indices: Option<[usize; 3]>,
}

#[derive(Debug, Clone)]
struct SceneShading {
    shader_name: String,
    material: SceneMaterial,
    diffuse_texture: Option<SceneTexture>,
}

#[derive(Debug, Clone)]
struct SceneMaterial {
    material_name: String,
    ambient: Vec3,
    diffuse: Vec3,
    specular: Vec3,
    emissive: Vec3,
    reflectivity: f64,
    opacity: f64,
}

#[derive(Debug, Clone)]
struct SceneTexture {
    texture_name: String,
    source_path: PathBuf,
    idtf_path: String,
}

#[derive(Debug, Clone)]
struct SceneNode {
    name: String,
    parent_index: Option<usize>,
    transform: TransformMatrix,
    mesh_index: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct TransformMatrix {
    rows: [[f64; 4]; 4],
}

impl TransformMatrix {
    const fn identity() -> Self {
        Self {
            rows: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }
}

#[derive(Debug, Clone)]
struct ScenePart {
    node_index: usize,
    resource_name: String,
    positions: Vec<Vec3>,
    triangles: Vec<SceneTriangle>,
    normals: Vec<Vec3>,
    texture_coords: Vec<Vec2>,
    shadings: Vec<SceneShading>,
}

#[derive(Debug, Clone)]
struct SceneMesh {
    bounds: Bounds,
    nodes: Vec<SceneNode>,
    parts: Vec<ScenePart>,
}

impl SceneMesh {
    fn bounds(&self) -> Bounds {
        self.bounds
    }

    fn vertex_count(&self) -> usize {
        self.parts.iter().map(|part| part.positions.len()).sum()
    }

    fn triangle_count(&self) -> usize {
        self.parts.iter().map(|part| part.triangles.len()).sum()
    }

    fn shading_count(&self) -> usize {
        self.parts.iter().map(|part| part.shadings.len()).sum()
    }

    fn textured_shader_count(&self) -> usize {
        self.parts
            .iter()
            .flat_map(|part| part.shadings.iter())
            .filter(|shading| shading.diffuse_texture.is_some())
            .count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Vec2 {
    x: f64,
    y: f64,
}

impl Vec2 {
    const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Vec3 {
    x: f64,
    y: f64,
    z: f64,
}

impl Vec3 {
    const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    const ONE: Self = Self::new(1.0, 1.0, 1.0);

    const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    fn scaled(self, scale: f64) -> Self {
        Self::new(self.x * scale, self.y * scale, self.z * scale)
    }

    fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    fn normalized_or(self, fallback: Self) -> Self {
        let length = self.length();

        if length <= f64::EPSILON {
            fallback
        } else {
            Self::new(self.x / length, self.y / length, self.z / length)
        }
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    min: Vec3,
    max: Vec3,
}

impl Bounds {
    fn empty() -> Self {
        Self {
            min: Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            max: Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        }
    }

    fn include(&mut self, point: Vec3) {
        self.min.x = self.min.x.min(point.x);
        self.min.y = self.min.y.min(point.y);
        self.min.z = self.min.z.min(point.z);
        self.max.x = self.max.x.max(point.x);
        self.max.y = self.max.y.max(point.y);
        self.max.z = self.max.z.max(point.z);
    }

    fn center(self) -> Vec3 {
        Vec3::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
            (self.min.z + self.max.z) * 0.5,
        )
    }

    fn radius(self) -> f64 {
        let size = self.max - self.min;
        size.x.max(size.y).max(size.z).max(1.0)
    }
}

#[derive(Debug, Clone, Copy)]
enum RotationOrder {
    Xyz,
    Xzy,
    Yzx,
    Yxz,
    Zxy,
    Zyx,
}

impl RotationOrder {
    fn from_fbx_value(value: Option<&FbxProperty>) -> Option<Self> {
        match property_i32(value)? {
            0 => Some(Self::Xyz),
            1 => Some(Self::Xzy),
            2 => Some(Self::Yzx),
            3 => Some(Self::Yxz),
            4 => Some(Self::Zxy),
            5 => Some(Self::Zyx),
            _ => None,
        }
    }

    fn apply(self, point: Vec3, rotation_deg: Vec3) -> Vec3 {
        let x = rotation_x(point, rotation_deg.x.to_radians());
        let y = rotation_y(point, rotation_deg.y.to_radians());
        let z = rotation_z(point, rotation_deg.z.to_radians());

        match self {
            Self::Xyz => rotation_z(rotation_y(x, rotation_deg.y.to_radians()), rotation_deg.z.to_radians()),
            Self::Xzy => rotation_y(rotation_z(x, rotation_deg.z.to_radians()), rotation_deg.y.to_radians()),
            Self::Yzx => rotation_x(rotation_z(y, rotation_deg.z.to_radians()), rotation_deg.x.to_radians()),
            Self::Yxz => rotation_z(rotation_x(y, rotation_deg.x.to_radians()), rotation_deg.z.to_radians()),
            Self::Zxy => rotation_y(rotation_x(z, rotation_deg.x.to_radians()), rotation_deg.y.to_radians()),
            Self::Zyx => rotation_x(rotation_y(z, rotation_deg.y.to_radians()), rotation_deg.x.to_radians()),
        }
    }
}

fn rotation_x(point: Vec3, radians: f64) -> Vec3 {
    let (sin, cos) = radians.sin_cos();
    Vec3::new(point.x, point.y * cos - point.z * sin, point.y * sin + point.z * cos)
}

fn rotation_y(point: Vec3, radians: f64) -> Vec3 {
    let (sin, cos) = radians.sin_cos();
    Vec3::new(point.x * cos + point.z * sin, point.y, -point.x * sin + point.z * cos)
}

fn rotation_z(point: Vec3, radians: f64) -> Vec3 {
    let (sin, cos) = radians.sin_cos();
    Vec3::new(point.x * cos - point.y * sin, point.x * sin + point.y * cos, point.z)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use fbx::{File as FbxFile, Node as FbxNode, Property as FbxProperty};

    use tempfile::tempdir;

    use super::*;

    fn sample_cli(input: PathBuf, converter: PathBuf) -> Cli {
        Cli {
            input,
            output: None,
            overwrite: false,
            units_scale: 1.0,
            backend: Backend::Idtf,
            idtf_converter: Some(converter),
            dry_run: true,
        }
    }

    fn sample_converter_path() -> PathBuf {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Users\\ajones\\AppData\\Local"))
            .join(LOCAL_U3D_CONVERTER_SUBPATH)
    }

    #[test]
    fn bundled_idtf_converter_path_uses_installed_executable_directory() {
        let temp_dir = tempdir().expect("temp dir");
        let install_dir = temp_dir.path().join("FBX2U3D");
        fs::create_dir_all(&install_dir).expect("install dir");

        let bundled = bundled_idtf_converter_path(&install_dir.join("fbx2u3d.exe")).expect("bundled path");

        assert_eq!(
            bundled,
            install_dir.join("u3d-sdk").join("U3D_A_061228_5").join("Bin").join("Win32").join("Release").join("IDTFConverter.exe")
        );
    }

    fn synthetic_mesh_file() -> FbxFile {
        FbxFile {
            version: fbx::Version::V7400,
            children: vec![
                FbxNode {
                    name: "Objects".to_owned(),
                    properties: Vec::new(),
                    children: vec![
                        FbxNode {
                            name: "Geometry".to_owned(),
                            properties: vec![
                                FbxProperty::I64(1001),
                                FbxProperty::String("Geometry::Box".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![
                                FbxNode {
                                    name: "Vertices".to_owned(),
                                    properties: vec![FbxProperty::F64Array(vec![
                                        0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0,
                                        0.0,
                                    ])],
                                    children: Vec::new(),
                                },
                                FbxNode {
                                    name: "PolygonVertexIndex".to_owned(),
                                    properties: vec![FbxProperty::I32Array(vec![0, 1, 2, -4])],
                                    children: Vec::new(),
                                },
                            ],
                        },
                        FbxNode {
                            name: "Model".to_owned(),
                            properties: vec![
                                FbxProperty::I64(2001),
                                FbxProperty::String("Model::Box".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![FbxNode {
                                name: "Properties70".to_owned(),
                                properties: Vec::new(),
                                children: vec![
                                    properties70_vec3("Lcl Translation", [2.0, 3.0, 4.0]),
                                    properties70_vec3("Lcl Rotation", [0.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Scaling", [1.0, 1.0, 1.0]),
                                ],
                            }],
                        },
                    ],
                },
                FbxNode {
                    name: "Connections".to_owned(),
                    properties: Vec::new(),
                    children: vec![FbxNode {
                        name: "C".to_owned(),
                        properties: vec![
                            FbxProperty::String("OO".to_owned()),
                            FbxProperty::I64(1001),
                            FbxProperty::I64(2001),
                        ],
                        children: Vec::new(),
                    }],
                },
            ],
        }
    }

    fn synthetic_textured_mesh_file(texture_file_name: &str) -> FbxFile {
        FbxFile {
            version: fbx::Version::V7400,
            children: vec![
                FbxNode {
                    name: "Objects".to_owned(),
                    properties: Vec::new(),
                    children: vec![
                        FbxNode {
                            name: "Geometry".to_owned(),
                            properties: vec![
                                FbxProperty::I64(1001),
                                FbxProperty::String("Geometry::Panel".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![
                                FbxNode {
                                    name: "Vertices".to_owned(),
                                    properties: vec![FbxProperty::F64Array(vec![
                                        0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0,
                                        0.0,
                                    ])],
                                    children: Vec::new(),
                                },
                                FbxNode {
                                    name: "PolygonVertexIndex".to_owned(),
                                    properties: vec![FbxProperty::I32Array(vec![0, 1, 2, -4])],
                                    children: Vec::new(),
                                },
                                layer_element_material_node(),
                                layer_element_uv_node(),
                            ],
                        },
                        FbxNode {
                            name: "Model".to_owned(),
                            properties: vec![
                                FbxProperty::I64(2001),
                                FbxProperty::String("Model::Panel".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![FbxNode {
                                name: "Properties70".to_owned(),
                                properties: Vec::new(),
                                children: vec![
                                    properties70_vec3("Lcl Translation", [0.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Rotation", [0.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Scaling", [1.0, 1.0, 1.0]),
                                ],
                            }],
                        },
                        FbxNode {
                            name: "Material".to_owned(),
                            properties: vec![
                                FbxProperty::I64(3001),
                                FbxProperty::String("Material::Paint".to_owned()),
                                FbxProperty::String(String::new()),
                            ],
                            children: vec![FbxNode {
                                name: "Properties70".to_owned(),
                                properties: Vec::new(),
                                children: vec![
                                    properties70_vec3("AmbientColor", [0.1, 0.2, 0.3]),
                                    properties70_vec3("DiffuseColor", [0.25, 0.5, 0.75]),
                                    properties70_vec3("SpecularColor", [0.6, 0.5, 0.4]),
                                    properties70_vec3("EmissiveColor", [0.05, 0.0, 0.0]),
                                    properties70_f64("ReflectionFactor", 0.2),
                                    properties70_f64("Opacity", 0.85),
                                ],
                            }],
                        },
                        FbxNode {
                            name: "Texture".to_owned(),
                            properties: vec![
                                FbxProperty::I64(4001),
                                FbxProperty::String("Texture::Diffuse".to_owned()),
                                FbxProperty::String(String::new()),
                            ],
                            children: Vec::new(),
                        },
                        FbxNode {
                            name: "Video".to_owned(),
                            properties: vec![
                                FbxProperty::I64(5001),
                                FbxProperty::String("Video::Diffuse".to_owned()),
                                FbxProperty::String(String::new()),
                            ],
                            children: vec![node_with_string_property("RelativeFilename", texture_file_name)],
                        },
                    ],
                },
                FbxNode {
                    name: "Connections".to_owned(),
                    properties: Vec::new(),
                    children: vec![
                        FbxNode {
                            name: "C".to_owned(),
                            properties: vec![
                                FbxProperty::String("OO".to_owned()),
                                FbxProperty::I64(1001),
                                FbxProperty::I64(2001),
                            ],
                            children: Vec::new(),
                        },
                        FbxNode {
                            name: "C".to_owned(),
                            properties: vec![
                                FbxProperty::String("OO".to_owned()),
                                FbxProperty::I64(3001),
                                FbxProperty::I64(2001),
                            ],
                            children: Vec::new(),
                        },
                        FbxNode {
                            name: "C".to_owned(),
                            properties: vec![
                                FbxProperty::String("OP".to_owned()),
                                FbxProperty::I64(4001),
                                FbxProperty::I64(3001),
                                FbxProperty::String("DiffuseColor".to_owned()),
                            ],
                            children: Vec::new(),
                        },
                        FbxNode {
                            name: "C".to_owned(),
                            properties: vec![
                                FbxProperty::String("OO".to_owned()),
                                FbxProperty::I64(5001),
                                FbxProperty::I64(4001),
                            ],
                            children: Vec::new(),
                        },
                    ],
                },
            ],
        }
    }

    fn properties70_vec3(name: &str, value: [f64; 3]) -> FbxNode {
        FbxNode {
            name: "P".to_owned(),
            properties: vec![
                FbxProperty::String(name.to_owned()),
                FbxProperty::String(name.to_owned()),
                FbxProperty::String(String::new()),
                FbxProperty::String("A".to_owned()),
                FbxProperty::F64(value[0]),
                FbxProperty::F64(value[1]),
                FbxProperty::F64(value[2]),
            ],
            children: Vec::new(),
        }
    }

    fn properties70_f64(name: &str, value: f64) -> FbxNode {
        FbxNode {
            name: "P".to_owned(),
            properties: vec![
                FbxProperty::String(name.to_owned()),
                FbxProperty::String(name.to_owned()),
                FbxProperty::String(String::new()),
                FbxProperty::String("A".to_owned()),
                FbxProperty::F64(value),
            ],
            children: Vec::new(),
        }
    }

    fn node_with_string_property(name: &str, value: &str) -> FbxNode {
        FbxNode {
            name: name.to_owned(),
            properties: vec![FbxProperty::String(value.to_owned())],
            children: Vec::new(),
        }
    }

    fn layer_element_material_node() -> FbxNode {
        FbxNode {
            name: "LayerElementMaterial".to_owned(),
            properties: Vec::new(),
            children: vec![
                node_with_string_property("MappingInformationType", "AllSame"),
                node_with_string_property("ReferenceInformationType", "IndexToDirect"),
                FbxNode {
                    name: "Materials".to_owned(),
                    properties: vec![FbxProperty::I32Array(vec![0])],
                    children: Vec::new(),
                },
            ],
        }
    }

    fn layer_element_uv_node() -> FbxNode {
        FbxNode {
            name: "LayerElementUV".to_owned(),
            properties: Vec::new(),
            children: vec![
                node_with_string_property("MappingInformationType", "ByPolygonVertex"),
                node_with_string_property("ReferenceInformationType", "IndexToDirect"),
                FbxNode {
                    name: "UV".to_owned(),
                    properties: vec![FbxProperty::F64Array(vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0])],
                    children: Vec::new(),
                },
                FbxNode {
                    name: "UVIndex".to_owned(),
                    properties: vec![FbxProperty::I32Array(vec![0, 1, 2, 3])],
                    children: Vec::new(),
                },
            ],
        }
    }

    fn layer_element_normal_node() -> FbxNode {
        FbxNode {
            name: "LayerElementNormal".to_owned(),
            properties: Vec::new(),
            children: vec![
                node_with_string_property("MappingInformationType", "ByPolygonVertex"),
                node_with_string_property("ReferenceInformationType", "Direct"),
                FbxNode {
                    name: "Normals".to_owned(),
                    properties: vec![FbxProperty::F64Array(vec![
                        0.0, 0.6, 0.8, 0.0, 0.6, 0.8, 0.0, 0.6, 0.8, 0.0, 0.6, 0.8,
                    ])],
                    children: Vec::new(),
                },
            ],
        }
    }

    fn synthetic_smooth_normal_file() -> FbxFile {
        FbxFile {
            version: fbx::Version::V7400,
            children: vec![
                FbxNode {
                    name: "Objects".to_owned(),
                    properties: Vec::new(),
                    children: vec![
                        FbxNode {
                            name: "Geometry".to_owned(),
                            properties: vec![
                                FbxProperty::I64(1001),
                                FbxProperty::String("Geometry::SmoothPanel".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![
                                FbxNode {
                                    name: "Vertices".to_owned(),
                                    properties: vec![FbxProperty::F64Array(vec![
                                        0.0, 0.0, 0.0,
                                        1.0, 0.0, 0.0,
                                        1.0, 1.0, 0.0,
                                        0.0, 1.0, 0.0,
                                    ])],
                                    children: Vec::new(),
                                },
                                FbxNode {
                                    name: "PolygonVertexIndex".to_owned(),
                                    properties: vec![FbxProperty::I32Array(vec![0, 1, 2, -4])],
                                    children: Vec::new(),
                                },
                                layer_element_normal_node(),
                            ],
                        },
                        FbxNode {
                            name: "Model".to_owned(),
                            properties: vec![
                                FbxProperty::I64(2001),
                                FbxProperty::String("Model::SmoothPanel".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![FbxNode {
                                name: "Properties70".to_owned(),
                                properties: Vec::new(),
                                children: vec![
                                    properties70_vec3("Lcl Translation", [0.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Rotation", [0.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Scaling", [1.0, 1.0, 1.0]),
                                ],
                            }],
                        },
                    ],
                },
                FbxNode {
                    name: "Connections".to_owned(),
                    properties: Vec::new(),
                    children: vec![FbxNode {
                        name: "C".to_owned(),
                        properties: vec![
                            FbxProperty::String("OO".to_owned()),
                            FbxProperty::I64(1001),
                            FbxProperty::I64(2001),
                        ],
                        children: Vec::new(),
                    }],
                },
            ],
        }
    }

    fn write_minimal_tga(path: &Path) {
        let mut bytes = vec![0_u8; 18];
        bytes[2] = 2;
        bytes[12] = 1;
        bytes[14] = 1;
        bytes[16] = 24;
        bytes.extend_from_slice(&[0, 0, 255]);
        fs::write(path, bytes).expect("tga texture");
    }

    fn synthetic_hierarchy_file() -> FbxFile {
        FbxFile {
            version: fbx::Version::V7400,
            children: vec![
                FbxNode {
                    name: "Objects".to_owned(),
                    properties: Vec::new(),
                    children: vec![
                        FbxNode {
                            name: "Geometry".to_owned(),
                            properties: vec![
                                FbxProperty::I64(1001),
                                FbxProperty::String("Geometry::Body".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![
                                FbxNode {
                                    name: "Vertices".to_owned(),
                                    properties: vec![FbxProperty::F64Array(vec![
                                        0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0,
                                        0.0,
                                    ])],
                                    children: Vec::new(),
                                },
                                FbxNode {
                                    name: "PolygonVertexIndex".to_owned(),
                                    properties: vec![FbxProperty::I32Array(vec![0, 1, 2, -4])],
                                    children: Vec::new(),
                                },
                            ],
                        },
                        FbxNode {
                            name: "Model".to_owned(),
                            properties: vec![
                                FbxProperty::I64(2001),
                                FbxProperty::String("Model::Body".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![FbxNode {
                                name: "Properties70".to_owned(),
                                properties: Vec::new(),
                                children: vec![
                                    properties70_vec3("Lcl Translation", [2.0, 3.0, 4.0]),
                                    properties70_vec3("Lcl Rotation", [0.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Scaling", [1.0, 1.0, 1.0]),
                                ],
                            }],
                        },
                        FbxNode {
                            name: "Model".to_owned(),
                            properties: vec![
                                FbxProperty::I64(3001),
                                FbxProperty::String("Model::Assembly".to_owned()),
                                FbxProperty::String("Null".to_owned()),
                            ],
                            children: vec![FbxNode {
                                name: "Properties70".to_owned(),
                                properties: Vec::new(),
                                children: vec![
                                    properties70_vec3("Lcl Translation", [10.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Rotation", [0.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Scaling", [1.0, 1.0, 1.0]),
                                ],
                            }],
                        },
                    ],
                },
                FbxNode {
                    name: "Connections".to_owned(),
                    properties: Vec::new(),
                    children: vec![
                        FbxNode {
                            name: "C".to_owned(),
                            properties: vec![
                                FbxProperty::String("OO".to_owned()),
                                FbxProperty::I64(1001),
                                FbxProperty::I64(2001),
                            ],
                            children: Vec::new(),
                        },
                        FbxNode {
                            name: "C".to_owned(),
                            properties: vec![
                                FbxProperty::String("OO".to_owned()),
                                FbxProperty::I64(2001),
                                FbxProperty::I64(3001),
                            ],
                            children: Vec::new(),
                        },
                    ],
                },
            ],
        }
    }

    fn synthetic_instanced_geometry_file() -> FbxFile {
        FbxFile {
            version: fbx::Version::V7400,
            children: vec![
                FbxNode {
                    name: "Objects".to_owned(),
                    properties: Vec::new(),
                    children: vec![
                        FbxNode {
                            name: "Geometry".to_owned(),
                            properties: vec![
                                FbxProperty::I64(1001),
                                FbxProperty::String("Geometry::SharedBody".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![
                                FbxNode {
                                    name: "Vertices".to_owned(),
                                    properties: vec![FbxProperty::F64Array(vec![
                                        0.0, 0.0, 0.0,
                                        1.0, 0.0, 0.0,
                                        1.0, 1.0, 0.0,
                                        0.0, 1.0, 0.0,
                                    ])],
                                    children: Vec::new(),
                                },
                                FbxNode {
                                    name: "PolygonVertexIndex".to_owned(),
                                    properties: vec![FbxProperty::I32Array(vec![0, 1, 2, -4])],
                                    children: Vec::new(),
                                },
                            ],
                        },
                        FbxNode {
                            name: "Model".to_owned(),
                            properties: vec![
                                FbxProperty::I64(2001),
                                FbxProperty::String("Model::BodyA".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![FbxNode {
                                name: "Properties70".to_owned(),
                                properties: Vec::new(),
                                children: vec![
                                    properties70_vec3("Lcl Translation", [2.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Rotation", [0.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Scaling", [1.0, 1.0, 1.0]),
                                ],
                            }],
                        },
                        FbxNode {
                            name: "Model".to_owned(),
                            properties: vec![
                                FbxProperty::I64(2002),
                                FbxProperty::String("Model::BodyB".to_owned()),
                                FbxProperty::String("Mesh".to_owned()),
                            ],
                            children: vec![FbxNode {
                                name: "Properties70".to_owned(),
                                properties: Vec::new(),
                                children: vec![
                                    properties70_vec3("Lcl Translation", [6.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Rotation", [0.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Scaling", [1.0, 1.0, 1.0]),
                                ],
                            }],
                        },
                        FbxNode {
                            name: "Model".to_owned(),
                            properties: vec![
                                FbxProperty::I64(3001),
                                FbxProperty::String("Model::Assembly".to_owned()),
                                FbxProperty::String("Null".to_owned()),
                            ],
                            children: vec![FbxNode {
                                name: "Properties70".to_owned(),
                                properties: Vec::new(),
                                children: vec![
                                    properties70_vec3("Lcl Translation", [10.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Rotation", [0.0, 0.0, 0.0]),
                                    properties70_vec3("Lcl Scaling", [1.0, 1.0, 1.0]),
                                ],
                            }],
                        },
                    ],
                },
                FbxNode {
                    name: "Connections".to_owned(),
                    properties: Vec::new(),
                    children: vec![
                        FbxNode {
                            name: "C".to_owned(),
                            properties: vec![
                                FbxProperty::String("OO".to_owned()),
                                FbxProperty::I64(1001),
                                FbxProperty::I64(2001),
                            ],
                            children: Vec::new(),
                        },
                        FbxNode {
                            name: "C".to_owned(),
                            properties: vec![
                                FbxProperty::String("OO".to_owned()),
                                FbxProperty::I64(1001),
                                FbxProperty::I64(2002),
                            ],
                            children: Vec::new(),
                        },
                        FbxNode {
                            name: "C".to_owned(),
                            properties: vec![
                                FbxProperty::String("OO".to_owned()),
                                FbxProperty::I64(2001),
                                FbxProperty::I64(3001),
                            ],
                            children: Vec::new(),
                        },
                        FbxNode {
                            name: "C".to_owned(),
                            properties: vec![
                                FbxProperty::String("OO".to_owned()),
                                FbxProperty::I64(2002),
                                FbxProperty::I64(3001),
                            ],
                            children: Vec::new(),
                        },
                    ],
                },
            ],
        }
    }

    #[test]
    fn build_plan_uses_u3d_extension_by_default() {
        let temp_dir = tempdir().expect("temp dir");
        let input = temp_dir.path().join("gearbox.FBX");
        let converter = temp_dir.path().join("IDTFConverter.exe");
        fs::write(&input, b"fbx").expect("input file");
        fs::write(&converter, b"converter").expect("converter file");

        let plan = build_plan(&sample_cli(input, converter.clone())).expect("plan");

        assert_eq!(plan.output.file_name().and_then(|name| name.to_str()), Some("gearbox.u3d"));
        assert_eq!(plan.idtf_converter, converter);
    }

    #[test]
    fn build_plan_rejects_non_fbx_inputs() {
        let temp_dir = tempdir().expect("temp dir");
        let input = temp_dir.path().join("gearbox.obj");
        let converter = temp_dir.path().join("IDTFConverter.exe");
        fs::write(&input, b"obj").expect("input file");
        fs::write(&converter, b"converter").expect("converter file");

        let error = build_plan(&sample_cli(input.clone(), converter)).expect_err("expected error");

        assert!(matches!(error, AppError::UnsupportedInput(path) if path == input));
    }

    #[test]
    fn build_plan_requires_overwrite_for_existing_output() {
        let temp_dir = tempdir().expect("temp dir");
        let input = temp_dir.path().join("gearbox.fbx");
        let output = temp_dir.path().join("gearbox.u3d");
        let converter = temp_dir.path().join("IDTFConverter.exe");
        fs::write(&input, b"fbx").expect("input file");
        fs::write(&output, b"u3d").expect("output file");
        fs::write(&converter, b"converter").expect("converter file");

        let mut cli = sample_cli(input, converter);
        cli.output = Some(output.clone());
        cli.dry_run = false;

        let error = build_plan(&cli).expect_err("expected error");

        assert!(matches!(error, AppError::OutputExists(path) if path == output));
    }

    #[test]
    fn build_plan_resolves_relative_output_against_input_directory() {
        let temp_dir = tempdir().expect("temp dir");
        let input = temp_dir.path().join("gearbox.fbx");
        let converter = temp_dir.path().join("IDTFConverter.exe");
        let output_dir = temp_dir.path().join("exports");
        fs::write(&input, b"fbx").expect("input file");
        fs::write(&converter, b"converter").expect("converter file");
        fs::create_dir(&output_dir).expect("output directory");

        let mut cli = sample_cli(input.clone(), converter);
        cli.output = Some(PathBuf::from("exports/gearbox.u3d"));

        let plan = build_plan(&cli).expect("plan");

        assert!(plan.output.is_absolute());
        assert!(plan.output.ends_with(Path::new("exports/gearbox.u3d")));
    }

    #[test]
    fn extract_scene_mesh_applies_model_translation() {
        let scene = extract_scene_mesh(&synthetic_mesh_file(), Path::new("synthetic.fbx"), 2.0).expect("scene");

        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].positions[0], Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(scene.parts[0].positions[2], Vec3::new(2.0, 2.0, 0.0));
        assert_eq!(
            scene.parts[0]
                .triangles
                .iter()
                .map(|triangle| triangle.position_indices)
                .collect::<Vec<_>>(),
            vec![[0, 1, 2], [0, 2, 3]]
        );
        assert!(scene.parts[0].texture_coords.is_empty());
        assert_eq!(scene.nodes.len(), 1);
        assert_eq!(scene.nodes[0].name, "Box");
        assert_eq!(scene.nodes[0].mesh_index, Some(0));
        assert_eq!(scene.nodes[0].transform.rows[3], [4.0, 6.0, 8.0, 1.0]);
        assert_eq!(scene.bounds().min, Vec3::new(4.0, 6.0, 8.0));
        assert_eq!(scene.bounds().max, Vec3::new(6.0, 8.0, 8.0));
    }

    #[test]
    fn extract_scene_mesh_preserves_parent_group_hierarchy() {
        let scene = extract_scene_mesh(&synthetic_hierarchy_file(), Path::new("assembly.fbx"), 1.0).expect("scene");

        assert_eq!(scene.nodes.len(), 2);
        assert_eq!(scene.nodes[0].name, "Assembly");
        assert_eq!(scene.nodes[0].mesh_index, None);
        assert_eq!(scene.nodes[1].name, "Body");
        assert_eq!(scene.nodes[1].parent_index, Some(0));
        assert_eq!(scene.nodes[1].mesh_index, Some(0));
        assert_eq!(scene.nodes[1].transform.rows[3], [2.0, 3.0, 4.0, 1.0]);
        assert_eq!(scene.bounds().min, Vec3::new(12.0, 3.0, 4.0));
        assert_eq!(scene.bounds().max, Vec3::new(13.0, 4.0, 4.0));
    }

    #[test]
    fn extract_scene_mesh_preserves_source_normals_when_present() {
        let scene = extract_scene_mesh(&synthetic_smooth_normal_file(), Path::new("smooth.fbx"), 1.0).expect("scene");

        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].normals.len(), 6);
        assert_eq!(scene.parts[0].normals[0], Vec3::new(0.0, 0.6, 0.8));
        assert_eq!(scene.parts[0].triangles[0].normal_indices, [0, 1, 2]);
    }

    #[test]
    fn extract_scene_mesh_preserves_multiple_instances_of_shared_geometry() {
        let scene = extract_scene_mesh(&synthetic_instanced_geometry_file(), Path::new("assembly.fbx"), 1.0)
            .expect("scene");

        assert_eq!(scene.nodes.len(), 3);
        assert_eq!(scene.parts.len(), 2);
        assert_eq!(scene.nodes[0].name, "Assembly");
        assert_eq!(scene.nodes[1].name, "BodyA");
        assert_eq!(scene.nodes[2].name, "BodyB");
        assert_eq!(scene.nodes[1].parent_index, Some(0));
        assert_eq!(scene.nodes[2].parent_index, Some(0));
        assert_eq!(scene.nodes[1].mesh_index, Some(0));
        assert_eq!(scene.nodes[2].mesh_index, Some(1));
        assert_eq!(scene.parts[0].positions, scene.parts[1].positions);
        assert_eq!(scene.bounds().min, Vec3::new(12.0, 0.0, 0.0));
        assert_eq!(scene.bounds().max, Vec3::new(17.0, 1.0, 0.0));
    }

    #[test]
    fn write_idtf_document_contains_mesh_counts() {
        let temp_dir = tempdir().expect("temp dir");
        let idtf_path = temp_dir.path().join("scene.idtf");
        let scene = extract_scene_mesh(&synthetic_mesh_file(), Path::new("synthetic.fbx"), 1.0).expect("scene");

        write_idtf_document(&idtf_path, &scene, Path::new("box.fbx")).expect("idtf");

        let idtf = fs::read_to_string(idtf_path).expect("idtf content");
        assert!(idtf.contains("NODE_NAME \"Box\""));
        assert!(idtf.contains("MODEL_POSITION_COUNT 4"));
        assert!(idtf.contains("FACE_COUNT 2"));
        assert!(idtf.contains("RESOURCE_NAME \"Box_Mesh\""));
        assert!(idtf.contains("RESOURCE_LIST \"MODEL\""));
    }

    #[test]
    fn write_idtf_document_preserves_group_hierarchy() {
        let temp_dir = tempdir().expect("temp dir");
        let idtf_path = temp_dir.path().join("scene.idtf");
        let scene = extract_scene_mesh(&synthetic_hierarchy_file(), Path::new("assembly.fbx"), 1.0).expect("scene");

        write_idtf_document(&idtf_path, &scene, Path::new("assembly.fbx")).expect("idtf");

        let idtf = fs::read_to_string(idtf_path).expect("idtf content");
        assert!(idtf.contains("NODE \"GROUP\" {"));
        assert!(idtf.contains("NODE_NAME \"Assembly\""));
        assert!(idtf.contains("NODE_NAME \"Body\""));
        assert!(idtf.contains("PARENT_NAME \"Assembly\""));
    }

    #[test]
    fn write_idtf_document_adds_notice_root_model() {
        let temp_dir = tempdir().expect("temp dir");
        let idtf_path = temp_dir.path().join("scene.idtf");
        let scene = extract_scene_mesh(&synthetic_hierarchy_file(), Path::new("assembly.fbx"), 1.0).expect("scene");

        write_idtf_document(&idtf_path, &scene, Path::new("assembly.fbx")).expect("idtf");

        let idtf = fs::read_to_string(idtf_path).expect("idtf content");
        let expected_parent = format!(
            "NODE \"GROUP\" {{\n\tNODE_NAME \"Assembly\"\n\tPARENT_LIST {{\n\t\tPARENT_COUNT 1\n\t\tPARENT 0 {{\n\t\t\tPARENT_NAME \"{}\"",
            FBX2U3D_NOTICE_NODE_NAME
        );

        assert!(idtf.contains(&format!("NODE \"MODEL\" {{\n\tNODE_NAME \"{}\"", FBX2U3D_NOTICE_NODE_NAME)));
        assert!(idtf.contains(&format!("NODE_NAME \"{}\"", FBX2U3D_NOTICE_NODE_NAME)));
        assert!(idtf.contains(&format!("RESOURCE_NAME \"{}\"", FBX2U3D_NOTICE_MESH_NAME)));
        assert!(idtf.contains(&expected_parent));
    }

    #[test]
    fn write_idtf_document_uses_source_normals_when_present() {
        let temp_dir = tempdir().expect("temp dir");
        let idtf_path = temp_dir.path().join("scene.idtf");
        let scene = extract_scene_mesh(&synthetic_smooth_normal_file(), Path::new("smooth.fbx"), 1.0)
            .expect("scene");

        write_idtf_document(&idtf_path, &scene, Path::new("smooth.fbx")).expect("idtf");

        let idtf = fs::read_to_string(idtf_path).expect("idtf content");
        assert!(idtf.contains("MODEL_NORMAL_COUNT 6"));
        assert!(idtf.contains("0.000000 0.600000 0.800000"));
        assert!(idtf.contains("MESH_FACE_NORMAL_LIST"));
    }

    #[test]
    fn write_idtf_document_keeps_multiple_model_nodes_for_shared_geometry_instances() {
        let temp_dir = tempdir().expect("temp dir");
        let idtf_path = temp_dir.path().join("scene.idtf");
        let scene = extract_scene_mesh(&synthetic_instanced_geometry_file(), Path::new("assembly.fbx"), 1.0)
            .expect("scene");

        write_idtf_document(&idtf_path, &scene, Path::new("assembly.fbx")).expect("idtf");

        let idtf = fs::read_to_string(idtf_path).expect("idtf content");
        assert!(idtf.contains("\tRESOURCE_COUNT 3\n\tRESOURCE 0 {\n\t\tRESOURCE_NAME \"BodyA_Mesh\""));
        assert!(idtf.contains("\tRESOURCE 1 {\n\t\tRESOURCE_NAME \"BodyB_Mesh\""));
        assert!(idtf.contains(&format!("\tRESOURCE 2 {{\n\t\tRESOURCE_NAME \"{}\"", FBX2U3D_NOTICE_MESH_NAME)));
        assert!(idtf.contains("NODE_NAME \"BodyA\""));
        assert!(idtf.contains("NODE_NAME \"BodyB\""));
        assert!(idtf.contains("PARENT_NAME \"Assembly\""));
    }

    #[test]
    fn extract_scene_mesh_exports_material_color_and_texture_data() {
        let temp_dir = tempdir().expect("temp dir");
        let source_path = temp_dir.path().join("scene.fbx");
        let texture_path = temp_dir.path().join("panel.tga");
        write_minimal_tga(&texture_path);

        let scene = extract_scene_mesh(&synthetic_textured_mesh_file("panel.tga"), &source_path, 1.0).expect("scene");
        let part = &scene.parts[0];

        assert_eq!(part.shadings.len(), 1);
        assert_eq!(part.texture_coords.len(), 6);
        assert_eq!(part.shadings[0].material.diffuse, Vec3::new(0.25, 0.5, 0.75));
        assert_eq!(part.shadings[0].material.opacity, 0.85);
        assert_eq!(
            part.shadings[0]
                .diffuse_texture
                .as_ref()
                .expect("diffuse texture")
                .source_path,
            texture_path
        );
        assert!(part.triangles.iter().all(|triangle| triangle.texture_coord_indices.is_some()));
    }

    #[test]
    fn write_idtf_document_contains_texture_sections() {
        let temp_dir = tempdir().expect("temp dir");
        let source_path = temp_dir.path().join("scene.fbx");
        let texture_path = temp_dir.path().join("panel.tga");
        let idtf_path = temp_dir.path().join("scene.idtf");
        write_minimal_tga(&texture_path);

        let scene = extract_scene_mesh(&synthetic_textured_mesh_file("panel.tga"), &source_path, 1.0).expect("scene");
        let staged_scene = stage_scene_assets(&scene, temp_dir.path()).expect("staged scene");
        write_idtf_document(&idtf_path, &staged_scene, Path::new("box.fbx")).expect("idtf");

        let idtf = fs::read_to_string(idtf_path).expect("idtf content");
        assert!(idtf.contains("RESOURCE_LIST \"TEXTURE\""));
        assert!(idtf.contains("MODEL_TEXTURE_COORD_COUNT 6"));
        assert!(idtf.contains("MESH_FACE_TEXTURE_COORD_LIST"));
        assert!(idtf.contains("SHADER_ACTIVE_TEXTURE_COUNT 1"));
        assert!(idtf.contains("MATERIAL_DIFFUSE 0.250000 0.500000 0.750000"));
    }

    #[test]
    fn ensure_unique_scene_resource_names_deduplicates_shader_material_and_texture_names() {
        let mut scene = SceneMesh {
            bounds: Bounds::empty(),
            nodes: vec![
                SceneNode {
                    name: "RepeatedNode".to_owned(),
                    parent_index: None,
                    transform: TransformMatrix::identity(),
                    mesh_index: Some(0),
                },
                SceneNode {
                    name: "RepeatedNode".to_owned(),
                    parent_index: Some(0),
                    transform: TransformMatrix::identity(),
                    mesh_index: Some(1),
                },
            ],
            parts: vec![
                ScenePart {
                    node_index: 0,
                    resource_name: "RepeatedMesh".to_owned(),
                    positions: Vec::new(),
                    triangles: Vec::new(),
                    normals: Vec::new(),
                    texture_coords: Vec::new(),
                    shadings: vec![SceneShading {
                        shader_name: "RepeatedShader".to_owned(),
                        material: SceneMaterial {
                            material_name: "RepeatedMaterial".to_owned(),
                            ambient: Vec3::ZERO,
                            diffuse: Vec3::ZERO,
                            specular: Vec3::ZERO,
                            emissive: Vec3::ZERO,
                            reflectivity: 0.0,
                            opacity: 1.0,
                        },
                        diffuse_texture: Some(SceneTexture {
                            texture_name: "RepeatedTexture".to_owned(),
                            source_path: PathBuf::from("first.png"),
                            idtf_path: "first.png".to_owned(),
                        }),
                    }],
                },
                ScenePart {
                    node_index: 1,
                    resource_name: "RepeatedMesh".to_owned(),
                    positions: Vec::new(),
                    triangles: Vec::new(),
                    normals: Vec::new(),
                    texture_coords: Vec::new(),
                    shadings: vec![SceneShading {
                        shader_name: "RepeatedShader".to_owned(),
                        material: SceneMaterial {
                            material_name: "RepeatedMaterial".to_owned(),
                            ambient: Vec3::ZERO,
                            diffuse: Vec3::ZERO,
                            specular: Vec3::ZERO,
                            emissive: Vec3::ZERO,
                            reflectivity: 0.0,
                            opacity: 1.0,
                        },
                        diffuse_texture: Some(SceneTexture {
                            texture_name: "RepeatedTexture".to_owned(),
                            source_path: PathBuf::from("second.png"),
                            idtf_path: "second.png".to_owned(),
                        }),
                    }],
                },
            ],
        };

        ensure_unique_scene_resource_names(&mut scene);

        assert_eq!(scene.nodes[0].name, "RepeatedNode");
        assert_eq!(scene.nodes[1].name, "RepeatedNode_2");
        assert_eq!(scene.parts[0].resource_name, "RepeatedMesh");
        assert_eq!(scene.parts[1].resource_name, "RepeatedMesh_2");
        assert_eq!(scene.parts[0].shadings[0].shader_name, "RepeatedShader");
        assert_eq!(scene.parts[1].shadings[0].shader_name, "RepeatedShader_2");
        assert_eq!(scene.parts[0].shadings[0].material.material_name, "RepeatedMaterial");
        assert_eq!(scene.parts[1].shadings[0].material.material_name, "RepeatedMaterial_2");
        assert_eq!(
            scene.parts[0].shadings[0]
                .diffuse_texture
                .as_ref()
                .expect("first texture")
                .texture_name,
            "RepeatedTexture"
        );
        assert_eq!(
            scene.parts[1].shadings[0]
                .diffuse_texture
                .as_ref()
                .expect("second texture")
                .texture_name,
            "RepeatedTexture_2"
        );
    }

    #[test]
    fn converter_round_trip_works_when_sdk_is_installed() {
        let converter = sample_converter_path();
        if !converter.is_file() {
            return;
        }

        let temp_dir = tempdir().expect("temp dir");
        let idtf_path = temp_dir.path().join("scene.idtf");
        let output_path = temp_dir.path().join("scene.u3d");
        let scene = extract_scene_mesh(&synthetic_mesh_file(), Path::new("synthetic.fbx"), 1.0).expect("scene");

        write_idtf_document(&idtf_path, &scene, Path::new("box.fbx")).expect("idtf");
        run_idtf_converter(&converter, &idtf_path, &output_path).expect("converter output");

        assert!(output_path.is_file());
        assert!(fs::metadata(output_path).expect("metadata").len() > 0);
    }

    #[test]
    fn converter_round_trip_preserves_notice_name_when_sdk_is_installed() {
        let converter = sample_converter_path();
        if !converter.is_file() {
            return;
        }

        let temp_dir = tempdir().expect("temp dir");
        let idtf_path = temp_dir.path().join("scene.idtf");
        let output_path = temp_dir.path().join("scene.u3d");
        let scene = extract_scene_mesh(&synthetic_mesh_file(), Path::new("synthetic.fbx"), 1.0).expect("scene");

        write_idtf_document(&idtf_path, &scene, Path::new("box.fbx")).expect("idtf");
        run_idtf_converter(&converter, &idtf_path, &output_path).expect("converter output");

        let u3d_bytes = fs::read(output_path).expect("u3d bytes");
        let u3d_text = String::from_utf8_lossy(&u3d_bytes);

        assert!(u3d_text.contains(FBX2U3D_NOTICE_NODE_NAME));
    }

    #[test]
    fn textured_converter_round_trip_works_when_sdk_is_installed() {
        let converter = sample_converter_path();
        if !converter.is_file() {
            return;
        }

        let temp_dir = tempdir().expect("temp dir");
        let source_path = temp_dir.path().join("scene.fbx");
        let texture_path = temp_dir.path().join("panel.tga");
        let idtf_path = temp_dir.path().join("scene.idtf");
        let output_path = temp_dir.path().join("scene.u3d");
        write_minimal_tga(&texture_path);

        let scene = extract_scene_mesh(&synthetic_textured_mesh_file("panel.tga"), &source_path, 1.0).expect("scene");
        let staged_scene = stage_scene_assets(&scene, temp_dir.path()).expect("staged scene");
        write_idtf_document(&idtf_path, &staged_scene, &source_path).expect("idtf");
        run_idtf_converter(&converter, &idtf_path, &output_path).expect("converter output");

        assert!(output_path.is_file());
        assert!(fs::metadata(output_path).expect("metadata").len() > 0);
    }
}