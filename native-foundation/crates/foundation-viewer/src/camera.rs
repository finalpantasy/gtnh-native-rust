use glam::{Mat4, Vec3};
use voxel_core::coords::CubePos;
use voxel_core::{
    coords::BlockPos,
    raycast::{Point, Ray, UnknownPolicy},
};
pub type Error = String;
#[derive(Clone, Debug, PartialEq)]
pub struct GridPoint {
    cube: CubePos,
    local: [f64; 3],
}
impl GridPoint {
    pub fn new(cube: CubePos, local: [f64; 3]) -> Result<Self, Error> {
        if cube.block_at(0).is_none()
            || local
                .iter()
                .any(|v| !v.is_finite() || !(0. ..16.).contains(v))
        {
            return Err("invalid camera grid position".into());
        }
        Ok(Self { cube, local })
    }
    pub fn cube(&self) -> CubePos {
        self.cube
    }
    pub fn local(&self) -> [f64; 3] {
        self.local
    }
    pub fn translate(&mut self, delta: [f64; 3]) -> Result<(), Error> {
        if delta.iter().any(|v| !v.is_finite() || v.abs() > 1_000_000.) {
            return Err("camera movement exceeds finite step bounds".into());
        }
        let sum = std::array::from_fn::<_, 3, _>(|i| self.local[i] + delta[i]);
        let shifts = sum.map(|v| (v / 16.).floor() as i64);
        let cube = self
            .cube
            .offset(shifts)
            .ok_or("camera coordinate overflow")?;
        let local = std::array::from_fn(|i| sum[i] - shifts[i] as f64 * 16.);
        *self = Self::new(cube, local)?;
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    focus: GridPoint,
    yaw: f32,
    pitch: f32,
    distance: f32,
}
impl Camera {
    pub fn home() -> Self {
        Self {
            focus: GridPoint::new(CubePos::new(-1, 1_000_000, 0), [8.; 3]).unwrap(),
            yaw: 0.67,
            pitch: 0.47,
            distance: 98.,
        }
    }
    pub fn focus(&self) -> &GridPoint {
        &self.focus
    }
    pub fn set_focus(&mut self, focus: GridPoint) {
        self.focus = focus;
    }
    pub fn orbit(&mut self, yaw: f32, pitch: f32) -> Result<(), Error> {
        if !yaw.is_finite() || !pitch.is_finite() {
            return Err("nonfinite orbit input".into());
        }
        self.yaw = (self.yaw + yaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + pitch).clamp(-1.35, 1.35);
        Ok(())
    }
    pub fn zoom(&mut self, scroll: f32) -> Result<(), Error> {
        if !scroll.is_finite() {
            return Err("nonfinite zoom input".into());
        }
        self.distance = (self.distance * (-scroll.clamp(-20., 20.) * 0.1).exp()).clamp(12., 200.);
        Ok(())
    }
    pub fn pan(&mut self, right: f64, forward: f64, up: f64) -> Result<(), Error> {
        let yaw = f64::from(self.yaw);
        let (sin, cos) = yaw.sin_cos();
        self.focus.translate([
            right * cos - forward * sin,
            up,
            -right * sin - forward * cos,
        ])
    }
    pub fn matrix(&self) -> Mat4 {
        let focus = Vec3::from_array(self.focus.local.map(|v| v as f32));
        let horizontal = self.pitch.cos() * self.distance;
        let offset = Vec3::new(
            self.yaw.sin() * horizontal,
            self.pitch.sin() * self.distance,
            self.yaw.cos() * horizontal,
        );
        Mat4::perspective_rh(55f32.to_radians(), 4. / 3., 0.2, 500.)
            * Mat4::look_at_rh(focus + offset, focus, Vec3::Y)
    }
    pub fn distance(&self) -> f32 {
        self.distance
    }
    /// An inspection ray in the same camera-relative coordinates as the rendered scene.
    /// The absolute cube origin is never converted to floating point.
    pub fn ray(&self, ndc: [f32; 2]) -> Result<Ray, Error> {
        if ndc.iter().any(|v| !v.is_finite() || v.abs() > 1.) {
            return Err("inspection point is outside the world image".into());
        }
        let horizontal = self.pitch.cos() * self.distance;
        let offset = Vec3::new(
            self.yaw.sin() * horizontal,
            self.pitch.sin() * self.distance,
            self.yaw.cos() * horizontal,
        );
        let local_eye = Vec3::from_array(self.focus.local.map(|v| v as f32)) + offset;
        let near = self.matrix().inverse() * glam::Vec4::new(ndc[0], ndc[1], 0., 1.);
        let direction = near.truncate() / near.w - local_eye;
        let mut eye = self.focus.clone();
        eye.translate(offset.to_array().map(f64::from))?;
        let base = eye.cube.block_at(0).ok_or("inspection origin overflow")?;
        let integer = eye.local.map(|v| v.floor() as i64);
        let cell = BlockPos::new(
            base.x
                .checked_add(integer[0])
                .ok_or("inspection X overflow")?,
            base.y
                .checked_add(integer[1])
                .ok_or("inspection Y overflow")?,
            base.z
                .checked_add(integer[2])
                .ok_or("inspection Z overflow")?,
        );
        let fraction = std::array::from_fn(|i| eye.local[i] - integer[i] as f64);
        let origin = Point::new(cell, fraction).map_err(|e| e.to_string())?;
        let ray = Ray::new(origin, direction.to_array().map(f64::from), 500.)
            .map_err(|e| e.to_string())?;
        Ok(Ray {
            max_queries: 2048,
            unknown: UnknownPolicy::SkipForInspection,
            ..ray
        })
    }
    pub fn project_local(&self, point: [f32; 3]) -> Option<[f32; 2]> {
        let clip = self.matrix() * Vec3::from_array(point).extend(1.);
        if !clip.is_finite() || clip.w <= 0. {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if ndc.x.abs() > 1. || ndc.y.abs() > 1. || !(0. ..=1.).contains(&ndc.z) {
            None
        } else {
            Some([ndc.x, ndc.y])
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspection_markers_reject_points_outside_the_camera_depth_range() {
        let c = Camera::home();
        let focus = Vec3::from_array(c.focus.local.map(|v| v as f32));
        let toward_eye = Vec3::new(
            c.yaw.sin() * c.pitch.cos(),
            c.pitch.sin(),
            c.yaw.cos() * c.pitch.cos(),
        );
        let eye = focus + toward_eye * c.distance;
        assert!(c.project_local(focus.to_array()).is_some());
        assert!(
            c.project_local((eye - toward_eye * 0.05).to_array())
                .is_none()
        );
        assert!(
            c.project_local((eye - toward_eye * 600.).to_array())
                .is_none()
        );
        assert!(c.project_local((eye + toward_eye).to_array()).is_none());
    }
    #[test]
    fn fractional_movement_crosses_negative_cube_boundaries_without_global_float_conversion() {
        let mut p = GridPoint::new(CubePos::new(-1, -1, 0), [0.25, 15.75, 8.]).unwrap();
        p.translate([-0.5, 0.5, 16.]).unwrap();
        assert_eq!(p.cube(), CubePos::new(-2, 0, 1));
        assert_eq!(p.local(), [15.75, 0.25, 8.]);
    }
    #[test]
    fn extreme_coordinates_keep_local_camera_precision_and_reject_overflow_atomically() {
        let mut p = GridPoint::new(
            CubePos::new(i64::MAX / 16, i64::MIN / 16, 0),
            [15.75, 0.25, 8.],
        )
        .unwrap();
        let before = p.clone();
        assert!(p.translate([0.5, 0., 0.]).is_err());
        assert_eq!(p, before);
        assert!(p.translate([0., -0.5, 0.]).is_err());
        assert_eq!(p, before);
        assert!(p.translate([f64::NAN, 0., 0.]).is_err());
        assert_eq!(p, before);
        assert!(p.translate([1e30, 0., 0.]).is_err());
        assert_eq!(p, before);
        let home = Camera::home();
        let mut far = home.clone();
        far.set_focus(GridPoint::new(p.cube(), home.focus.local()).unwrap());
        assert_eq!(home.matrix(), far.matrix());
        assert_ne!(home.matrix(), Mat4::IDENTITY);
    }
    #[test]
    fn zoom_orbit_and_frame_controls_stay_finite_and_bounded() {
        let mut c = Camera::home();
        let initial = c.clone();
        assert!(c.orbit(f32::NAN, 0.).is_err());
        assert_eq!(c, initial);
        c.orbit(100., 100.).unwrap();
        c.zoom(10000.).unwrap();
        assert!((12. ..=200.).contains(&c.distance()));
        assert!(c.matrix().is_finite());
        let before = c.clone();
        assert!(c.pan(0., f64::INFINITY, 0.).is_err());
        assert_eq!(c, before);
        assert!(GridPoint::new(CubePos::new(0, 0, 0), [16., 0., 0.]).is_err());
    }
    #[test]
    fn projected_voxel_center_casts_back_to_the_same_actual_world_cell() {
        use voxel_core::{
            coords::BlockPos,
            raycast::{Outcome, cast},
        };
        let camera = Camera::home();
        let pixel = camera.project_local([8.5, 19.5, 8.5]).unwrap();
        let ray = camera.ray(pixel).unwrap();
        let base = camera.focus().cube().block_at(0).unwrap();
        let target = BlockPos::new(base.x + 8, base.y + 19, base.z + 8);
        match cast(ray, |p| Some(if p == target { 4 } else { 0 }))
            .unwrap()
            .outcome
        {
            Outcome::Hit(hit) => assert_eq!(hit.position, target),
            other => panic!("projected target missed: {other:?}"),
        }
    }
    #[test]
    fn camera_ray_translation_preserves_local_origin_and_direction_at_huge_offsets() {
        let home = Camera::home();
        let mut far = home.clone();
        far.set_focus(
            GridPoint::new(
                CubePos::new(i64::MIN / 16 + 100, i64::MAX / 16 - 100, -1_000_000),
                home.focus.local(),
            )
            .unwrap(),
        );
        let near_ray = home.ray([0.2, -0.1]).unwrap();
        let far_ray = far.ray([0.2, -0.1]).unwrap();
        let relative = |camera: &Camera, ray: Ray| {
            let base = camera.focus.cube().block_at(0).unwrap();
            let p = ray.origin.cell();
            [p.x - base.x, p.y - base.y, p.z - base.z]
        };
        assert_eq!(near_ray.direction, far_ray.direction);
        assert_eq!(near_ray.origin.fraction(), far_ray.origin.fraction());
        assert_eq!(relative(&home, near_ray), relative(&far, far_ray));
        assert!(home.ray([f32::NAN, 0.]).is_err());
        assert!(home.ray([1.1, 0.]).is_err());
        far.set_focus(GridPoint::new(CubePos::new(i64::MAX / 16, 0, 0), [15.5; 3]).unwrap());
        assert!(far.ray([0., 0.]).is_err());
    }
}
