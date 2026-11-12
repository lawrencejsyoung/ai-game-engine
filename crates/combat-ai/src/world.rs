//! The interface the host game implements so agents can query the world.

use glam::Vec3;

/// World queries the AI needs from the host engine.
///
/// Must be `Sync`: with the `parallel` feature, agents query the world from
/// multiple threads simultaneously. Queries are read-only.
pub trait WorldQuery: Sync {
    /// Returns `true` if nothing blocks a straight line between the two points
    /// (typically a physics raycast against static geometry and props).
    fn line_of_sight(&self, from: Vec3, to: Vec3) -> bool;

    /// Navigation distance from `from` to `to`, or `None` if `to` is unreachable
    /// or off the navmesh. The default assumes open ground (straight-line distance).
    ///
    /// Called only for a handful of shortlisted tactical candidates per decision,
    /// so a real path query is affordable here.
    fn path_distance(&self, from: Vec3, to: Vec3) -> Option<f32> {
        Some(from.distance(to))
    }
}
