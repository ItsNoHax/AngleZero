// Shared by tests/summit.rs and tests/city.rs: the title camera and a projection through it.

use angle_zero::track::Track;

/// Where a world point lands on the 480 × 272 screen from a camera, as the GE's perspective with
/// the game's field of view puts it, or `None` behind the camera.
pub fn project(cam: &angle_zero::camera::Camera, p: angle_zero::math::Vec3) -> Option<(f32, f32)> {
    let f = cam.look_at.sub(cam.pos).normalized();
    let r = angle_zero::math::Vec3::new(-f.z, 0.0, f.x).normalized();
    let u = angle_zero::math::Vec3::new(r.y * f.z - r.z * f.y, r.z * f.x - r.x * f.z, r.x * f.y - r.y * f.x);
    let d = p.sub(cam.pos);
    let (x, y, z) = (d.dot(r), d.dot(u), d.dot(f));
    if z < 0.5 {
        return None;
    }
    let ty = (cam.fov.to_radians() * 0.5).tan();
    let tx = ty * 16.0 / 9.0;
    Some((240.0 + x / z / tx * 240.0, 136.0 - y / z / ty * 136.0))
}

pub fn title_camera(t: &Track) -> angle_zero::camera::Camera {
    let (x, y, z, yaw, _) = angle_zero::track::carpark_parking(t);
    let car = angle_zero::vehicle::CarState { x, y, z, yaw, ..Default::default() };
    let mut cam = angle_zero::camera::Camera::new();
    cam.fov = angle_zero::camera::TITLE_FOV;
    cam.update_title(&car, 0.0);
    cam
}

