use std::{collections::HashMap, io::Cursor};

use arrow::{
    array::AsArray,
    datatypes::{Float64Type, UInt16Type, UInt8Type},
    ipc::reader::StreamReader,
};
use bevy::{
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task},
};
use bevy_aabb_instancing::{Cuboid, CuboidMaterialId, Cuboids, VertexPullingRenderPlugin};
use bevy_panorbit_camera::{PanOrbitCamera, PanOrbitCameraPlugin};
use futures_lite::future::{self, block_on};
use rstar::Envelope;

use crux_format::{ArrowPointCloud, Point, PointCloudTrait, PointTrait, AABB};

const HOST: &str = "0.0.0.0";
const PORT: &str = "3000";
const COLLECTION: &str = "default";
const COLOR_ATTRIBUTE: &str = "z";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    App::new()
        .insert_resource(SpatialReference::default())
        .insert_resource(PointCache::default())
        .add_plugins((
            DefaultPlugins,
            FrameTimeDiagnosticsPlugin,
            LogDiagnosticsPlugin::default(),
            PanOrbitCameraPlugin,
            VertexPullingRenderPlugin::default(),
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, load_controll_system)
        .add_systems(Update, spawn_load_task)
        .add_systems(Update, handle_load_task)
        .add_systems(Update, update)
        .add_systems(Update, camera_controls_system)
        .run();
}

fn setup(mut commands: Commands) {
    // camera
    commands.spawn((Camera3dBundle::default(), PanOrbitCamera::default()));

    // text
    commands.spawn((
        TextBundle::from_section("Debug text!", TextStyle::default()).with_style(Style {
            position_type: PositionType::Absolute,
            top: Val::Px(5.0),
            left: Val::Px(15.0),
            ..default()
        }),
        DebugText,
    ));

    // cuboids
    commands
        .spawn(SpatialBundle::default())
        .insert((Cuboids::default(), CuboidMaterialId(0)));
}

fn update(
    cache: Res<PointCache>,
    mut sr: ResMut<SpatialReference>,
    mut cuboids: Query<&mut Cuboids>,
) {
    if cache.is_changed() && cache.data.contains_key(COLLECTION) {
        let pc = cache.data.get(COLLECTION).unwrap();
        let aabb: AABB<Point<f32, 3>> = pc.aabb();

        let offset = if let Some(o) = sr.origin {
            // TODO: update sr
            o
        } else {
            // set origin to center
            let center = aabb.center();
            let p = Vec3::from_slice(center.coords());
            sr.origin = Some(p);
            sr.camera = p;
            p
        };

        // generate instances
        let num_points = pc.num_points();
        let mut instances = Vec::with_capacity(num_points);
        info!("Generating {num_points} instances");

        // color
        let colors = match (
            COLOR_ATTRIBUTE,
            pc.schema().column_with_name(COLOR_ATTRIBUTE).is_some(),
        ) {
            ("classification", true) => pc
                .store
                .iter()
                .flat_map(|e| pc.store.batches(e.key()))
                .flat_map(|batch| {
                    batch
                        .column_by_name(COLOR_ATTRIBUTE)
                        .unwrap()
                        .as_primitive::<UInt8Type>()
                        .values()
                        .iter()
                        .map(|v| match v {
                            0 => Color::GRAY,
                            1 => Color::BEIGE,
                            2 => Color::OLIVE,
                            3 => Color::LIME_GREEN,
                            4 => Color::GREEN,
                            5 => Color::DARK_GREEN,
                            6 => Color::MAROON,
                            9 => Color::BLUE,
                            11 => Color::DARK_GRAY,
                            _ => Color::ORANGE,
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
            ("intensity", true) => pc
                .store
                .iter()
                .flat_map(|e| pc.store.batches(e.key()))
                .flat_map(|batch| {
                    batch
                        .column_by_name(COLOR_ATTRIBUTE)
                        .unwrap()
                        .as_primitive::<UInt16Type>()
                        .values()
                        .iter()
                        .map(|v| {
                            let intensity = *v as f32 / 255.;
                            Color::rgba(intensity, intensity, intensity, 1.)
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
            ("z", true) => {
                let zmin = rstar::Point::nth(&aabb.lower(), 2);
                let zmax = rstar::Point::nth(&aabb.upper(), 2);

                let gradient = colorgrad::turbo();

                pc.store
                    .iter()
                    .flat_map(|e| pc.store.batches(e.key()))
                    .flat_map(|batch| {
                        batch
                            .column_by_name(COLOR_ATTRIBUTE)
                            .unwrap()
                            .as_primitive::<Float64Type>()
                            .values()
                            .iter()
                            .map(|v| {
                                let position = (*v as f32 - zmin) / (zmax - zmin);
                                let color = gradient.at(position as f64);

                                Color::rgba(
                                    color.r as f32,
                                    color.g as f32,
                                    color.b as f32,
                                    color.a as f32,
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect()
            }
            (attribute, true) => {
                eprintln!("No color for attribute `{attribute}` defined, fallback color used!");
                vec![Color::ORANGE; pc.num_points()]
            }
            (attribute, false) => {
                eprintln!("No attribute `{attribute}` found, fallback color used!");
                vec![Color::ORANGE; pc.num_points()]
            }
        };

        for (i, p) in pc.points::<Point<f32, 3>>().enumerate() {
            // shift to origin
            let p = Vec3::from_slice(p.coords()) - offset;

            // Convert from easting (x) northing (y) up (z) to right hand y up (bevy)
            //
            //     z y                y
            //     |/                 |
            //     0 –– x    ===>     0 –– x
            //                       /
            //                      z
            //
            let p = Vec3::from_slice(&[p.x, p.z, -p.y]);

            let half_extents = (aabb.area() / num_points as f32).powf(1. / 3.) / 10. * Vec3::ONE;

            let min = p - half_extents;
            let max = p + half_extents;
            let color = colors[i].as_rgba_u32();
            let mut cuboid = Cuboid::new(min, max, color);
            cuboid.set_depth_bias(0);
            instances.push(cuboid);
        }

        cuboids.get_single_mut().unwrap().instances = instances;
    }
}

#[derive(Resource, Default)]
struct PointCache {
    queue: Vec<String>,
    data: HashMap<String, ArrowPointCloud>,
}

#[derive(Component)]
struct LoadTask(Task<ArrowPointCloud>);

fn spawn_load_task(mut commands: Commands, mut cache: ResMut<PointCache>) {
    if !cache.queue.is_empty() {
        let thread_pool = AsyncComputeTaskPool::get();

        for url in cache.queue.iter().cloned() {
            // Spawn new task on the AsyncComputeTaskPool; the task will be
            // executed in the background, and the Task future returned by
            // spawn() can be used to poll for the result
            let task = thread_pool.spawn(async move {
                // get pointcloud
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .build()
                    .unwrap();

                let response = rt.block_on(reqwest::get(url)).unwrap();

                let body = rt.block_on(response.bytes()).unwrap();
                let cursor = Cursor::new(body);
                let reader = StreamReader::try_new(cursor, None).unwrap();

                reader.into()
            });

            // Spawn new entity and add our new task as a component
            commands.spawn(LoadTask(task));
        }

        cache.queue.clear();
    }
}

fn handle_load_task(
    mut commands: Commands,
    mut load_tasks: Query<(Entity, &mut LoadTask)>,
    mut cache: ResMut<PointCache>,
) {
    for (entity, mut task) in &mut load_tasks {
        if let Some(pc) = block_on(future::poll_once(&mut task.0)) {
            cache.data.insert(COLLECTION.to_string(), pc);

            // Task is complete, so remove task component from entity
            commands.entity(entity).remove::<LoadTask>();
        }
    }
}

fn load_controll_system(
    key_input: Res<Input<KeyCode>>,
    mut cache: ResMut<PointCache>,
    sr: Res<SpatialReference>,
    camera: Query<&PanOrbitCamera>,
) {
    // get p=0.0001
    if key_input.just_pressed(KeyCode::F5) {
        cache
            .queue
            .push(format!("http://{HOST}:{PORT}/points?p=0.0001"));
    }

    // get p=0.001
    if key_input.just_pressed(KeyCode::F4) {
        cache
            .queue
            .push(format!("http://{HOST}:{PORT}/points?p=0.001"));
    }
    // get p=0.01
    if key_input.just_pressed(KeyCode::F3) {
        cache
            .queue
            .push(format!("http://{HOST}:{PORT}/points?p=0.01"));
    }
    // get p=0.1
    if key_input.just_pressed(KeyCode::F2) {
        cache
            .queue
            .push(format!("http://{HOST}:{PORT}/points?p=0.1"));
    }
    // get full dataset
    if key_input.just_pressed(KeyCode::F1) {
        cache.queue.push(format!("http://{HOST}:{PORT}/points"));
    }
    // update
    if key_input.just_pressed(KeyCode::U) {
        let camera = camera.get_single().unwrap();
        let radius = camera.radius.unwrap_or(1.);

        let lower = sr.camera - radius / 2.;
        let upper = sr.camera + radius / 2.;

        let query = format!(
            "http://{HOST}:{PORT}/points?bounds={},{},{},{},{},{},0,{}",
            lower.x,
            lower.y,
            lower.z,
            upper.x,
            upper.y,
            upper.z,
            1. / radius.sqrt() / 1000.
        );
        cache.queue.push(query);
    }
}

#[derive(Resource, Default)]
struct SpatialReference {
    origin: Option<Vec3>,
    camera: Vec3,
}

#[derive(Component)]
struct DebugText;

// Press 'R' to reset the camera
fn camera_controls_system(
    key_input: Res<Input<KeyCode>>,
    mut camera: Query<&mut PanOrbitCamera>,
    mut query: Query<&mut Text, With<DebugText>>,
    cache: Res<PointCache>,
    mut sr: ResMut<SpatialReference>,
    mut gizmos: Gizmos,
) {
    let mut camera = camera.get_single_mut().unwrap();

    // camera debug text
    let mut text = query.get_single_mut().unwrap();
    text.sections[0].value = [
        "Camera parameters",
        &format!(
            "Focus: [{:.3}, {:.3}, {:.3}]",
            camera.focus[0], camera.focus[1], camera.focus[2]
        ),
        &format!("Alpha: {:.3}", camera.alpha.unwrap_or_default()),
        &format!("Beta: {:.3}", camera.beta.unwrap_or_default()),
        &format!("Radius: {:.3}", camera.radius.unwrap_or_default()),
        &format!(
            "Focus in SRS: [{:.3}, {:.3}, {:.3}]",
            sr.camera[0], sr.camera[1], sr.camera[2]
        ),
        &format!(
            "Data Origin: [{:.3}, {:.3}, {:.3}]",
            sr.origin.map(|p| p[0]).unwrap_or(f32::NAN),
            sr.origin.map(|p| p[1]).unwrap_or(f32::NAN),
            sr.origin.map(|p| p[2]).unwrap_or(f32::NAN)
        ),
    ]
    .join("\n");

    // camera reset
    if key_input.just_pressed(KeyCode::R) {
        let aabb: AABB<Point<f32, 3>> = cache
            .data
            .values()
            .map(|pc| pc.aabb())
            .reduce(|acc, aabb| acc.merged(&aabb))
            .unwrap_or_else(AABB::new_empty);

        let dx = aabb.upper().x() - aabb.lower().x();
        let dy = aabb.upper().y() - aabb.lower().y();
        let dz = aabb.upper().z() - aabb.lower().z();

        camera.target_focus = Vec3::from_slice(&[0., -dy.max(dz) / 10., dy.max(dz) / 10.]);
        camera.target_alpha = 0.;
        camera.target_beta = 0.8;
        camera.target_radius = dx.max(dy);

        let center = aabb.center();
        let center = Vec3::from_slice(center.coords());
        sr.origin = Some(center);
        sr.camera = center;
    }

    // adjust origin from focus
    if camera.is_changed() {
        if let Some(mut o) = sr.origin {
            o.x += camera.focus.x;
            o.y += -camera.focus.z;
            o.z += camera.focus.y;

            sr.camera = o;
        }
    }

    // display query box
    let radius = camera.radius.unwrap_or(1.);
    gizmos.cuboid(
        Transform::from_translation(camera.focus).with_scale(Vec3::splat(radius)),
        Color::WHITE,
    );
}
