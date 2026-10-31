//! Dense contiguous cubic rendering and neighbor-change correctness fixture.
use crate::{
    factory,
    gpu::{Error, Gpu, write_png},
    voxel_mesh::{self, Surface},
    world_scene::{Meshing, Scene},
    world_storage::{Backend, WorldStore},
};
use glam::{Mat4, Vec3};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use voxel_core::{
    coords::CubePos,
    record::CubeRecord,
    storage::CubeStorage,
    streaming::{Limits, Streamer, Tickets},
};
const CENTER: CubePos = CubePos::new(-1, 1_000_000, 0);
fn focus(world: &mut Streamer<WorldStore>, all: bool, keep_neighbors: bool) -> Result<(), Error> {
    for job in world.set_tickets(Tickets::around(
        CENTER,
        if all { [1; 3] } else { [0; 3] },
        if keep_neighbors { [1; 3] } else { [0; 3] },
    )?) {
        assert!(world.acknowledge_canceled(job));
    }
    for _ in 0..16 {
        let jobs = world.plan(4, 27)?.loads;
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            let cube = world.read_or_generate(job, |_| {
                unreachable!("all dense fixture cubes must already exist")
            })?;
            assert!(world.complete(job, cube)?);
        }
    }
    Ok(())
}
fn camera(front: bool) -> Mat4 {
    Mat4::perspective_rh(55f32.to_radians(), 4. / 3., 0.2, 500.)
        * Mat4::look_at_rh(
            if front {
                Vec3::new(8.5, 8.5, 98.)
            } else {
                Vec3::new(62., 52., 76.)
            },
            Vec3::new(8.5, 8.5, 8.5),
            Vec3::Y,
        )
}
fn compare_surfaces(a: &[u8], b: &[u8], width: u32, height: u32) -> Result<usize, &'static str> {
    let expected = u64::from(width) * u64::from(height) * 4;
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || a.len() as u64 != expected
        || b.len() != a.len()
    {
        return Err("invalid comparison image dimensions");
    }
    let background = [6, 10, 14, 255];
    if !a.chunks_exact(4).any(|p| p != background) || !b.chunks_exact(4).any(|p| p != background) {
        return Err("comparison geometry is blank");
    }
    let mut differences = 0;
    for (index, (pa, pb)) in a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate() {
        if pa == pb {
            continue;
        }
        differences += 1;
        if differences > 8 || (pa != background && pb != background) {
            return Err("interior material or excessive silhouette differences");
        }
        let material = if pa == background { pb } else { pa };
        let x = (index % width as usize) as i32;
        let y = (index / width as usize) as i32;
        for pixels in [a, b] {
            let (mut saw_background, mut saw_material) = (false, false);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let at = (ny as usize * width as usize + nx as usize) * 4;
                    let p = &pixels[at..at + 4];
                    saw_background |= p == background;
                    saw_material |= p == material;
                }
            }
            if !saw_background || !saw_material {
                return Err("coverage difference away from shared one-pixel silhouette");
            }
        }
    }
    Ok(differences)
}
pub fn run(output: &Path) -> Result<Value, Error> {
    let root = factory::world_root(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())?;
    let mut storage = WorldStore::open(&root, Backend::Journal)?;
    let tickets = Tickets::around(CENTER, [1; 3], [1; 3])?;
    let mut cubes = Vec::new();
    for pos in tickets.positions() {
        let mut cube = CubeRecord::new(pos)?;
        for index in 0..4096 {
            cube.set(index, if pos == CENTER { 5 } else { 2 })?;
        }
        cubes.push(cube);
    }
    let reference_bytes = cubes.iter().map(voxel_mesh::mesh_size).sum::<usize>();
    assert!(reference_bytes > 1024 * 1024);
    storage.save_batch(&cubes.iter().collect::<Vec<_>>())?;
    let mut world = Streamer::new(
        storage,
        Limits {
            max_cubes: 27,
            max_resident_bytes: 27 * 32768,
            max_cube_bytes: 32768,
            max_pending_jobs: 4,
        },
    )?;
    focus(&mut world, true, true)?;
    let mut scene = Scene::new()?;
    scene.render_camera(&world, CENTER, camera(false))?;
    let image = scene.gpu().read_texture(scene.color_texture())?;
    write_png(
        &output.with_extension("png"),
        Scene::dimensions().0,
        Scene::dimensions().1,
        &image,
    )?;
    let mut payload = 0usize;
    for pos in world.resident_positions() {
        payload += Surface::rendered(&world, pos)?.size();
    }
    assert_eq!(payload, 54 * 6 * voxel_mesh::VERTEX_BYTES);

    // Keep all neighbors resident/simulated but hide them from rendering: center faces must reappear.
    focus(&mut world, false, true)?;
    assert_eq!(world.resident_positions().count(), 27);
    assert_eq!(
        Surface::rendered(&world, CENTER)?.size(),
        6 * 6 * voxel_mesh::VERTEX_BYTES
    );
    scene.render_camera(&world, CENTER, camera(false))?;
    let single = scene.gpu().read_texture(scene.color_texture())?;
    assert_ne!(single, image);
    let mut reference = Scene::with_meshing(Gpu::open()?, Meshing::BlockFacesReference)?;
    reference.render_camera(&world, CENTER, camera(false))?;
    let baseline = reference.gpu().read_texture(reference.color_texture())?;
    let reference_path = output.with_extension("reference.png");
    write_png(
        &reference_path,
        Scene::dimensions().0,
        Scene::dimensions().1,
        &baseline,
    )?;
    write_png(
        &output.with_extension("merged.png"),
        Scene::dimensions().0,
        Scene::dimensions().1,
        &single,
    )?;
    let silhouette_differences = compare_surfaces(
        &single,
        &baseline,
        Scene::dimensions().0,
        Scene::dimensions().1,
    )?;
    let reference_report = reference.finish(&reference_path)?;
    focus(&mut world, true, true)?;
    scene.render_camera(&world, CENTER, camera(false))?;
    assert!(
        image == scene.gpu().read_texture(scene.color_texture())?,
        "dense render changed after neighbor restore"
    );

    // Dig only inside this synthetic fixture. A hole in the front neighbor must reveal center material.
    scene.render_camera(&world, CENTER, camera(true))?;
    let opaque = scene.gpu().read_texture(scene.color_texture())?;
    let front = CENTER.offset([0, 0, 1]).unwrap();
    world.edit(front, |cube| {
        for z in 0..16 {
            cube.set(8 * 256 + z * 16 + 8, 0)?;
        }
        Ok(())
    })?;
    scene.render_camera(&world, CENTER, camera(true))?;
    let tunnel = scene.gpu().read_texture(scene.color_texture())?;
    assert_ne!(opaque, tunnel, "neighbor boundary edit left stale geometry");
    let (width, height) = Scene::dimensions();
    let center_pixel = ((height / 2 * width + width / 2) * 4) as usize;
    assert!(
        tunnel[center_pixel] > 100 && tunnel[center_pixel] > tunnel[center_pixel + 2],
        "neighbor hole did not reveal the orange center face"
    );
    let tunnel_path = output.with_extension("tunnel.png");
    write_png(&tunnel_path, width, height, &tunnel)?;
    focus(&mut world, false, false)?;
    assert_eq!(world.resident_positions().count(), 1);
    scene.render_camera(&world, CENTER, camera(false))?;
    assert!(
        single == scene.gpu().read_texture(scene.color_texture())?,
        "single render changed after unload"
    );
    focus(&mut world, true, true)?;
    scene.render_camera(&world, CENTER, camera(true))?;
    assert!(
        tunnel == scene.gpu().read_texture(scene.color_texture())?,
        "neighbor unload/reload changed persisted tunnel"
    );

    focus(&mut world, false, true)?;
    let entries = scene.usage().entries();
    world.edit(CENTER, |cube| {
        for index in 0..4096 {
            cube.set(index, 0)?;
        }
        Ok(())
    })?;
    scene.render_camera(&world, CENTER, camera(false))?;
    let empty = scene.gpu().read_texture(scene.color_texture())?;
    assert!(
        empty.chunks_exact(4).all(|p| p == &empty[..4]),
        "empty render retained old geometry"
    );
    assert_eq!(entries, scene.usage().entries());
    world.edit(CENTER, |cube| {
        for index in 0..4096 {
            cube.set(index, 5)?;
        }
        Ok(())
    })?;
    focus(&mut world, true, true)?;
    scene.render_camera(&world, CENTER, camera(true))?;
    assert!(
        tunnel == scene.gpu().read_texture(scene.color_texture())?,
        "center geometry failed to recover from air"
    );
    world.edit(CENTER, |cube| {
        for index in 0..4096 {
            let parity = (index & 15) + (index >> 8) + ((index >> 4) & 15);
            cube.set(index, if parity % 2 == 0 { 5 } else { 0 })?;
        }
        Ok(())
    })?;
    let rejected = scene
        .render_camera(&world, CENTER, camera(true))
        .unwrap_err();
    assert_eq!(
        rejected.downcast_ref::<residency::CacheError>(),
        Some(&residency::CacheError::TooLarge)
    );
    assert!(scene.usage().cpu() <= 1024 * 1024 && scene.usage().gpu() <= 1024 * 1024);
    Ok(
        json!({"status":"PASS","reference_per_block_bytes":reference_bytes,"merged_neighbor_bytes":payload,"single_cube_interior_gpu_pixels_identical":true,"single_cube_silhouette_pixel_differences":silhouette_differences,"neighbor_hide_restore_checked":true,"boundary_edit_unload_reload_checked":true,"empty_mesh_checked":true,
        "oversized_mesh_rejected_and_shutdown_clean":true,"world_directory":root,"comparison_reference_render":reference_report,"render":scene.finish(output)?}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_comparison_rejects_blank_material_and_interior_coverage_regressions() {
        let background = [6, 10, 14, 255];
        let material = [200, 80, 30, 255];
        let mut image = background.repeat(16 * 16);
        for y in 3..13 {
            for x in 3..13 {
                image[(y * 16 + x) * 4..(y * 16 + x + 1) * 4].copy_from_slice(&material);
            }
        }
        assert_eq!(compare_surfaces(&image, &image, 16, 16).unwrap(), 0);
        let mut edge = image.clone();
        edge[(3 * 16 + 3) * 4..(3 * 16 + 4) * 4].copy_from_slice(&background);
        assert_eq!(compare_surfaces(&image, &edge, 16, 16).unwrap(), 1);
        let mut hole = image.clone();
        hole[(8 * 16 + 8) * 4..(8 * 16 + 9) * 4].copy_from_slice(&background);
        assert!(compare_surfaces(&image, &hole, 16, 16).is_err());
        let mut changed = image.clone();
        changed[(8 * 16 + 8) * 4] = 10;
        assert!(compare_surfaces(&image, &changed, 16, 16).is_err());
        assert!(compare_surfaces(&image, &background.repeat(16 * 16), 16, 16).is_err());
        assert!(compare_surfaces(&image, &image[..16], 16, 16).is_err());
    }
}
