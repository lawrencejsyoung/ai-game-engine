//! Behavior tree leaf execution against an agent's brain.

use crate::agent::{ActorState, Stance};
use crate::brain::{Brain, FireMode, LookMode, Shared};
use crate::bt::{Blackboard, Condition, NodeId, NodeMem, Status, Task};
use crate::command::MoveSpeed;
use crate::math::{band, dist2_xz, flat_dir};
use crate::tactics::{self, Query, TacticalPosition};
use glam::Vec3;
use smallvec::SmallVec;

pub(crate) struct Ctx<'a, 'w> {
    pub b: &'a mut Brain,
    pub me: &'a ActorState,
    pub sh: &'a Shared<'w>,
    pub action: u16,
}

impl Ctx<'_, '_> {
    fn target_mem(&self) -> Option<&crate::memory::TargetMemory> {
        self.b.target.and_then(|t| self.b.memory.get(t))
    }

    fn threats(&self) -> SmallVec<[Vec3; 4]> {
        let mut v: SmallVec<[Vec3; 4]> = SmallVec::new();
        if let Some(t) = self.target_mem() {
            v.push(t.last_known_pos);
        }
        for e in self.b.memory.entries.iter().filter(|e| e.confirmed()) {
            if Some(e.target) != self.b.target && v.len() < 4 {
                v.push(e.last_known_pos);
            }
        }
        v
    }

    fn query<'q>(&self, threats: &'q [Vec3]) -> Query<'q> {
        let cfg = &self.b.archetype.config;
        let primary = self.target_mem().map(|t| t.last_known_pos);
        let attack_axis = match (
            primary,
            self.b.squad.and_then(|s| self.sh.squads.get(s.0 as usize)),
        ) {
            (Some(t), Some(sq)) if sq.members.len() >= 2 => {
                Some(flat_dir(sq.centroid - t, self.me.position - t))
            }
            _ => None,
        };
        Query {
            me: self.b.actor,
            pos: self.me.position,
            threats,
            primary,
            preferred: cfg.preferred_range,
            radius: cfg.tactical_search_radius,
            attack_axis,
        }
    }

    fn go(&mut self, p: Option<TacticalPosition>) -> Status {
        match p {
            Some(p) => {
                self.b.destination = Some(p.position);
                if p.cover != self.b.cover {
                    self.b.cover_since = self.sh.time;
                }
                self.b.cover = p.cover;
                Status::Success
            }
            None => Status::Failure,
        }
    }

    fn target_aim_point(&self) -> Option<Vec3> {
        let t = self.target_mem()?;
        Some(t.last_known_pos + Vec3::Y * 1.2)
    }
}

impl Blackboard for Ctx<'_, '_> {
    fn time(&self) -> f32 {
        self.sh.time
    }

    fn condition(&mut self, c: Condition) -> bool {
        let me = self.me;
        match c {
            Condition::HasTarget => self.b.target.is_some(),
            Condition::TargetVisible => self.target_mem().is_some_and(|t| t.visible),
            Condition::AmmoEmpty => me.ammo == 0,
            Condition::AmmoBelow(f) => me.ammo_fraction() < f,
            Condition::HealthBelow(f) => me.health_fraction() < f,
            Condition::InCover => self.b.in_cover(me, self.sh),
            Condition::HasDestination => self.b.destination.is_some(),
            Condition::HasAttackToken => self.b.token.is_some() && self.b.token == self.b.target,
            Condition::TargetInRange => {
                let cfg = &self.b.archetype.config;
                self.target_mem().is_some_and(|t| {
                    let d = t.last_known_pos.distance(me.position);
                    band(d, cfg.preferred_range.0, cfg.preferred_range.1, 0.0) > 0.0
                })
            }
        }
    }

    fn run_task(&mut self, t: Task, mem: &mut NodeMem, entered: bool) -> Status {
        let now = self.sh.time;
        let me = self.me;
        match t {
            Task::FindCover => {
                let threats = self.threats();
                if threats.is_empty() {
                    return Status::Failure;
                }
                let q = self.query(&threats);
                let r = tactics::find_cover(self.sh, &q);
                self.go(r)
            }
            Task::FindRetreatPosition => {
                let threats = self.threats();
                if threats.is_empty() {
                    return Status::Failure;
                }
                let q = self.query(&threats);
                let r = tactics::find_retreat(self.sh, &q);
                self.go(r)
            }
            Task::FindFlankPosition => {
                let threats = self.threats();
                let q = self.query(&threats);
                let r = tactics::find_flank(self.sh, &q, &mut self.b.rng);
                self.go(r)
            }
            Task::FindAttackPosition => {
                let threats = self.threats();
                let q = self.query(&threats);
                let Some(aim) = self.target_aim_point() else {
                    return Status::Failure;
                };
                let r = tactics::find_attack_position(self.sh, &q, aim, &mut self.b.rng);
                self.go(r)
            }
            Task::SetDestinationToLastKnown => {
                let dest = match self.target_mem() {
                    Some(t) if !t.visible => Some(t.last_known_pos + t.last_known_vel * 0.75),
                    Some(_) => None,
                    None => self.b.investigate.or_else(|| {
                        self.b
                            .memory
                            .entries
                            .iter()
                            .filter(|e| !e.confirmed() && !e.searched)
                            .max_by(|a, b| a.awareness.total_cmp(&b.awareness))
                            .map(|e| e.last_known_pos)
                    }),
                };
                match dest {
                    Some(d) => {
                        self.b.destination = Some(d);
                        self.b.cover = None;
                        Status::Success
                    }
                    None => Status::Failure,
                }
            }
            Task::FindPatrolPoint => {
                let cfg = &self.b.archetype.config;
                let (home, radius) = (self.b.home, cfg.patrol_radius);
                match tactics::find_patrol_point(
                    self.sh,
                    me.position,
                    home,
                    radius,
                    &mut self.b.rng,
                ) {
                    Some(p) => {
                        self.b.destination = Some(p);
                        self.b.cover = None;
                        Status::Success
                    }
                    None => Status::Failure,
                }
            }
            Task::MoveToDestination(speed) => {
                let Some(dest) = self.b.destination else {
                    return Status::Failure;
                };
                let arrive = self.b.archetype.config.arrive_radius;
                if entered {
                    // Generous stuck timeout based on distance.
                    mem.timer = now + me.position.distance(dest) / 1.5 + 4.0;
                }
                if dist2_xz(me.position, dest) <= arrive * arrive {
                    self.b.intent.move_to = None;
                    return Status::Success;
                }
                if now > mem.timer {
                    self.b.intent.move_to = None;
                    self.b.destination = None;
                    return Status::Failure;
                }
                let visible = self.target_mem().is_some_and(|t| t.visible);
                let i = &mut self.b.intent;
                i.move_to = Some(dest);
                i.speed = speed;
                i.stance = Stance::Stand;
                i.reload = false;
                if visible && speed != MoveSpeed::Sprint {
                    // Run and gun.
                    i.look = LookMode::Target;
                    i.fire = FireMode::Target;
                } else {
                    i.look = LookMode::Forward;
                    i.fire = FireMode::Hold;
                }
                Status::Running
            }
            Task::ShootTarget => {
                let Some(t) = self.target_mem().copied() else {
                    return Status::Success; // target gone
                };
                if me.ammo == 0 {
                    return Status::Failure;
                }
                let i = &mut self.b.intent;
                i.move_to = None;
                i.reload = false;
                i.look = LookMode::Target;
                if t.visible {
                    // Without an attack token the agent still shoots, just
                    // deliberately inaccurately (see aim.rs).
                    i.fire = FireMode::Target;
                    i.stance = Stance::Stand;
                    Status::Running
                } else if now - t.last_seen < 0.75 {
                    i.fire = FireMode::Hold;
                    Status::Running
                } else {
                    i.fire = FireMode::Hold;
                    Status::Failure
                }
            }
            Task::SuppressTarget(secs) => {
                if entered {
                    mem.timer = now + secs;
                }
                if me.ammo == 0 {
                    return Status::Failure;
                }
                let Some(p) = self.target_aim_point() else {
                    return Status::Failure;
                };
                let visible = self.target_mem().is_some_and(|t| t.visible);
                let i = &mut self.b.intent;
                i.move_to = None;
                i.stance = Stance::Stand;
                i.reload = false;
                if visible {
                    i.look = LookMode::Target;
                    i.fire = FireMode::Target;
                } else {
                    i.look = LookMode::Point(p);
                    i.fire = FireMode::Point(p);
                }
                if now >= mem.timer {
                    i.fire = FireMode::Hold;
                    Status::Success
                } else {
                    Status::Running
                }
            }
            Task::Reload => {
                if entered {
                    mem.timer = now + 6.0;
                }
                let i = &mut self.b.intent;
                i.fire = FireMode::Hold;
                if me.ammo >= me.mag_size {
                    i.reload = false;
                    return Status::Success;
                }
                if now > mem.timer {
                    i.reload = false;
                    return Status::Failure;
                }
                i.reload = true;
                Status::Running
            }
            Task::Search(secs) => {
                if entered {
                    mem.timer = now + secs;
                    mem.index = 0;
                }
                let i = &mut self.b.intent;
                i.move_to = None;
                i.fire = FireMode::Hold;
                // Sweep the view in a few different directions.
                let step = ((secs - (mem.timer - now)) / 0.9) as u16;
                if step != mem.index || entered {
                    mem.index = step;
                    let a = self.b.rng.range(0.0, std::f32::consts::TAU);
                    let p = me.eye() + Vec3::new(a.cos(), 0.0, a.sin()) * 10.0;
                    self.b.intent.look = LookMode::Point(p);
                }
                if now < mem.timer {
                    return Status::Running;
                }
                // Done: whatever we were looking for isn't here.
                self.b.investigate = None;
                let target = self.b.target;
                for e in self.b.memory.entries.iter_mut() {
                    if !e.visible && (Some(e.target) == target || !e.confirmed()) {
                        e.searched = true;
                        if !e.confirmed() {
                            e.awareness = 0.0;
                        }
                    }
                }
                Status::Success
            }
            Task::SetStance(s) => {
                self.b.intent.stance = s;
                Status::Success
            }
            Task::Bark(bark) => {
                self.b.bark(now, bark);
                Status::Success
            }
            Task::Hold => {
                let p = self.target_aim_point();
                let i = &mut self.b.intent;
                i.move_to = None;
                i.fire = FireMode::Hold;
                i.look = p.map_or(LookMode::Forward, LookMode::Point);
                Status::Running
            }
            Task::ClearDestination => {
                self.b.destination = None;
                self.b.intent.move_to = None;
                Status::Success
            }
        }
    }

    fn cooldown_get(&self, id: NodeId) -> f32 {
        self.b
            .cooldowns
            .iter()
            .find(|c| c.0 == self.action && c.1 == id)
            .map_or(f32::NEG_INFINITY, |c| c.2)
    }

    fn cooldown_set(&mut self, id: NodeId, ready_at: f32) {
        let a = self.action;
        if let Some(c) = self.b.cooldowns.iter_mut().find(|c| c.0 == a && c.1 == id) {
            c.2 = ready_at;
        } else {
            self.b.cooldowns.push((a, id, ready_at));
        }
    }
}
