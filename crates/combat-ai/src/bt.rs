//! Compact, arena-allocated behavior trees.
//!
//! Trees are authored with [`Bt`] builders, compiled once into a flat node
//! array, and shared (read-only) by every agent using the archetype. Per-agent
//! runtime state is a small `Vec<NodeMem>` indexed by node, reused across ticks,
//! so ticking a tree performs no allocation.
//!
//! Semantics:
//! - `Selector` is *reactive*: children are re-evaluated from the first each tick,
//!   so higher-priority branches interrupt running lower-priority ones.
//! - `Sequence` has *memory*: it resumes at the running child.
//! - A node not ticked on the previous tick is considered aborted; its memory is
//!   reset when it is entered again.

use crate::agent::Stance;
use crate::command::{Bark, MoveSpeed};

/// Result of ticking a node.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Finished successfully.
    Success,
    /// Finished unsuccessfully.
    Failure,
    /// Still in progress; tick again next think.
    Running,
}

/// Leaf conditions evaluated against the agent's blackboard.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Condition {
    /// A confirmed target exists.
    HasTarget,
    /// The primary target is in line of sight.
    TargetVisible,
    /// Magazine empty.
    AmmoEmpty,
    /// Magazine below the given fraction.
    AmmoBelow(f32),
    /// Health below the given fraction.
    HealthBelow(f32),
    /// Standing at a claimed cover point.
    InCover,
    /// A move destination is set.
    HasDestination,
    /// Holding an attack token on the primary target.
    HasAttackToken,
    /// Primary target is inside the preferred range band.
    TargetInRange,
}

/// Leaf tasks the engine knows how to execute.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Task {
    /// Choose and claim the best protective cover. Fails if none.
    FindCover,
    /// Choose a position on the primary target's flank. Fails if none.
    FindFlankPosition,
    /// Choose a position with line of sight to the target at a good range.
    FindAttackPosition,
    /// Choose a covered position away from threats.
    FindRetreatPosition,
    /// Set the destination to the target's last known position / investigate point.
    SetDestinationToLastKnown,
    /// Pick a random reachable patrol point around home.
    FindPatrolPoint,
    /// Move to the destination. Running until arrived.
    MoveToDestination(MoveSpeed),
    /// Aim and fire at the visible target. Running while engaged; fails once
    /// line of sight has been lost for a moment.
    ShootTarget,
    /// Fire at the target's last known position for the given seconds.
    SuppressTarget(f32),
    /// Reload; running until the magazine is full.
    Reload,
    /// Look around for the given seconds (search behavior), then mark the
    /// investigated position as searched.
    Search(f32),
    /// Change stance.
    SetStance(Stance),
    /// Play a voice line.
    Bark(Bark),
    /// Stop moving and keep watching the last known threat direction. Always running.
    Hold,
    /// Clear the move destination.
    ClearDestination,
}

/// Per-node runtime memory for one agent.
#[derive(Copy, Clone, Debug, Default)]
pub struct NodeMem {
    /// General-purpose timer (absolute simulation time).
    pub timer: f32,
    /// Running-child index for sequences.
    pub index: u16,
    /// Tick counter value when this node was last ticked.
    pub last_tick: u32,
}

/// Index of a node within a tree.
pub type NodeId = u16;

#[derive(Clone, Debug)]
enum Node {
    Sequence { first: u16, count: u16 },
    Selector { first: u16, count: u16 },
    Inverter(NodeId),
    Succeeder(NodeId),
    Cooldown { child: NodeId, seconds: f32 },
    Timeout { child: NodeId, seconds: f32 },
    Wait(f32),
    Condition(Condition),
    Task(Task),
}

/// A compiled behavior tree.
#[derive(Clone, Debug)]
pub struct BehaviorTree {
    nodes: Vec<Node>,
    children: Vec<NodeId>,
    root: NodeId,
}

/// Tree builder / authoring DSL.
#[derive(Clone, Debug)]
pub enum Bt {
    /// Runs children in order until one fails.
    Sequence(Vec<Bt>),
    /// Runs children in order until one doesn't fail.
    Selector(Vec<Bt>),
    /// Inverts Success/Failure.
    Inverter(Box<Bt>),
    /// Turns Failure into Success.
    Succeeder(Box<Bt>),
    /// Fails without ticking the child for `seconds` after it completes.
    Cooldown(Box<Bt>, f32),
    /// Fails the child if it runs longer than `seconds`.
    Timeout(Box<Bt>, f32),
    /// Running for `seconds`, then Success.
    Wait(f32),
    /// Condition leaf.
    Condition(Condition),
    /// Task leaf.
    Task(Task),
}

impl Bt {
    /// Sequence node.
    pub fn sequence(children: impl IntoIterator<Item = Bt>) -> Bt {
        Bt::Sequence(children.into_iter().collect())
    }
    /// Selector node.
    pub fn selector(children: impl IntoIterator<Item = Bt>) -> Bt {
        Bt::Selector(children.into_iter().collect())
    }
    /// Condition leaf.
    pub fn cond(c: Condition) -> Bt {
        Bt::Condition(c)
    }
    /// Negated condition leaf.
    pub fn not(c: Condition) -> Bt {
        Bt::Inverter(Box::new(Bt::Condition(c)))
    }
    /// Task leaf.
    pub fn task(t: Task) -> Bt {
        Bt::Task(t)
    }
    /// Wait leaf.
    pub fn wait(seconds: f32) -> Bt {
        Bt::Wait(seconds)
    }
    /// Wraps in a succeeder.
    pub fn optional(self) -> Bt {
        Bt::Succeeder(Box::new(self))
    }
    /// Wraps in a cooldown.
    pub fn cooldown(self, seconds: f32) -> Bt {
        Bt::Cooldown(Box::new(self), seconds)
    }
    /// Wraps in a timeout.
    pub fn timeout(self, seconds: f32) -> Bt {
        Bt::Timeout(Box::new(self), seconds)
    }

    /// Compiles into a flat [`BehaviorTree`].
    pub fn build(self) -> BehaviorTree {
        let mut t = BehaviorTree {
            nodes: Vec::new(),
            children: Vec::new(),
            root: 0,
        };
        t.root = t.compile(self);
        assert!(t.nodes.len() < u16::MAX as usize, "behavior tree too large");
        t
    }
}

impl BehaviorTree {
    fn compile(&mut self, spec: Bt) -> NodeId {
        let push = |t: &mut Self, n: Node| {
            t.nodes.push(n);
            (t.nodes.len() - 1) as NodeId
        };
        match spec {
            Bt::Sequence(cs) => {
                let ids: Vec<NodeId> = cs.into_iter().map(|c| self.compile(c)).collect();
                let first = self.children.len() as u16;
                self.children.extend_from_slice(&ids);
                push(
                    self,
                    Node::Sequence {
                        first,
                        count: ids.len() as u16,
                    },
                )
            }
            Bt::Selector(cs) => {
                let ids: Vec<NodeId> = cs.into_iter().map(|c| self.compile(c)).collect();
                let first = self.children.len() as u16;
                self.children.extend_from_slice(&ids);
                push(
                    self,
                    Node::Selector {
                        first,
                        count: ids.len() as u16,
                    },
                )
            }
            Bt::Inverter(c) => {
                let c = self.compile(*c);
                push(self, Node::Inverter(c))
            }
            Bt::Succeeder(c) => {
                let c = self.compile(*c);
                push(self, Node::Succeeder(c))
            }
            Bt::Cooldown(c, seconds) => {
                let child = self.compile(*c);
                push(self, Node::Cooldown { child, seconds })
            }
            Bt::Timeout(c, seconds) => {
                let child = self.compile(*c);
                push(self, Node::Timeout { child, seconds })
            }
            Bt::Wait(s) => push(self, Node::Wait(s)),
            Bt::Condition(c) => push(self, Node::Condition(c)),
            Bt::Task(t) => push(self, Node::Task(t)),
        }
    }

    /// Number of nodes (size of the per-agent memory array).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the tree has no nodes (never true for a built tree).
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Ticks the tree once. `mem` must have `self.len()` entries; `tick` must
    /// increase by one per call (it detects aborted branches).
    pub fn tick<B: Blackboard + ?Sized>(
        &self,
        bb: &mut B,
        mem: &mut [NodeMem],
        tick: u32,
    ) -> Status {
        debug_assert_eq!(mem.len(), self.nodes.len());
        self.run(self.root, bb, mem, tick)
    }

    fn run<B: Blackboard + ?Sized>(
        &self,
        id: NodeId,
        bb: &mut B,
        mem: &mut [NodeMem],
        tick: u32,
    ) -> Status {
        let i = id as usize;
        // Entering fresh (not ticked last tick) => reset this node's memory.
        let entered = mem[i].last_tick.wrapping_add(1) != tick || tick == 0;
        if entered {
            mem[i] = NodeMem::default();
        }
        mem[i].last_tick = tick;
        match self.nodes[i] {
            Node::Sequence { first, count } => {
                let mut c = mem[i].index;
                while c < count {
                    let child = self.children[(first + c) as usize];
                    match self.run(child, bb, mem, tick) {
                        Status::Success => c += 1,
                        Status::Running => {
                            mem[i].index = c;
                            return Status::Running;
                        }
                        Status::Failure => {
                            mem[i].index = 0;
                            return Status::Failure;
                        }
                    }
                }
                mem[i].index = 0;
                Status::Success
            }
            Node::Selector { first, count } => {
                for c in 0..count {
                    let child = self.children[(first + c) as usize];
                    match self.run(child, bb, mem, tick) {
                        Status::Failure => continue,
                        s => return s,
                    }
                }
                Status::Failure
            }
            Node::Inverter(c) => match self.run(c, bb, mem, tick) {
                Status::Success => Status::Failure,
                Status::Failure => Status::Success,
                Status::Running => Status::Running,
            },
            Node::Succeeder(c) => match self.run(c, bb, mem, tick) {
                Status::Running => Status::Running,
                _ => Status::Success,
            },
            Node::Cooldown { child, seconds } => {
                // Cooldowns must survive branch aborts and action switches, so their
                // expiry lives on the blackboard rather than in per-node memory.
                let now = bb.time();
                let ready_at = bb.cooldown_get(id);
                if now < ready_at {
                    return Status::Failure;
                }
                let s = self.run(child, bb, mem, tick);
                if s != Status::Running {
                    bb.cooldown_set(id, now + seconds);
                }
                s
            }
            Node::Timeout { child, seconds } => {
                if entered {
                    mem[i].timer = bb.time() + seconds;
                }
                if bb.time() >= mem[i].timer {
                    return Status::Failure;
                }
                self.run(child, bb, mem, tick)
            }
            Node::Wait(seconds) => {
                if entered {
                    mem[i].timer = bb.time() + seconds;
                }
                if bb.time() >= mem[i].timer {
                    Status::Success
                } else {
                    Status::Running
                }
            }
            Node::Condition(c) => {
                if bb.condition(c) {
                    Status::Success
                } else {
                    Status::Failure
                }
            }
            Node::Task(t) => bb.run_task(t, &mut mem[i], entered),
        }
    }
}

/// The agent-side interface a behavior tree runs against.
pub trait Blackboard {
    /// Current simulation time.
    fn time(&self) -> f32;
    /// Evaluates a condition.
    fn condition(&mut self, c: Condition) -> bool;
    /// Executes one tick of a task. `entered` is true on the first tick of a run.
    fn run_task(&mut self, t: Task, mem: &mut NodeMem, entered: bool) -> Status;
    /// Reads a cooldown expiry time for node `id` (persistent across aborts).
    fn cooldown_get(&self, id: NodeId) -> f32;
    /// Writes a cooldown expiry time for node `id`.
    fn cooldown_set(&mut self, id: NodeId, ready_at: f32);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestBb {
        time: f32,
        visible: bool,
        log: Vec<Task>,
        cooldowns: Vec<(NodeId, f32)>,
        reload_ticks: u32,
    }

    impl Blackboard for TestBb {
        fn time(&self) -> f32 {
            self.time
        }
        fn condition(&mut self, c: Condition) -> bool {
            match c {
                Condition::TargetVisible => self.visible,
                _ => false,
            }
        }
        fn run_task(&mut self, t: Task, _mem: &mut NodeMem, _entered: bool) -> Status {
            self.log.push(t);
            match t {
                Task::Reload => {
                    self.reload_ticks += 1;
                    if self.reload_ticks >= 3 {
                        Status::Success
                    } else {
                        Status::Running
                    }
                }
                Task::Hold | Task::ShootTarget => Status::Running,
                _ => Status::Success,
            }
        }
        fn cooldown_get(&self, id: NodeId) -> f32 {
            self.cooldowns
                .iter()
                .find(|c| c.0 == id)
                .map_or(f32::NEG_INFINITY, |c| c.1)
        }
        fn cooldown_set(&mut self, id: NodeId, ready_at: f32) {
            self.cooldowns.retain(|c| c.0 != id);
            self.cooldowns.push((id, ready_at));
        }
    }

    #[test]
    fn sequence_resumes_running_child() {
        let t = Bt::sequence([
            Bt::task(Task::Bark(Bark::Reloading)),
            Bt::task(Task::Reload),
            Bt::task(Task::ClearDestination),
        ])
        .build();
        let mut mem = vec![NodeMem::default(); t.len()];
        let mut bb = TestBb::default();
        let mut statuses = vec![];
        for tick in 1..=3 {
            statuses.push(t.tick(&mut bb, &mut mem, tick));
        }
        assert_eq!(
            statuses,
            [Status::Running, Status::Running, Status::Success]
        );
        // Bark only ran once: the sequence resumed at Reload.
        assert_eq!(
            bb.log,
            [
                Task::Bark(Bark::Reloading),
                Task::Reload,
                Task::Reload,
                Task::Reload,
                Task::ClearDestination
            ]
        );
    }

    #[test]
    fn reactive_selector_interrupts_lower_priority() {
        let t = Bt::selector([
            Bt::sequence([
                Bt::cond(Condition::TargetVisible),
                Bt::task(Task::ShootTarget),
            ]),
            Bt::task(Task::Hold),
        ])
        .build();
        let mut mem = vec![NodeMem::default(); t.len()];
        let mut bb = TestBb::default();
        t.tick(&mut bb, &mut mem, 1);
        assert_eq!(bb.log.last(), Some(&Task::Hold));
        bb.visible = true;
        t.tick(&mut bb, &mut mem, 2);
        assert_eq!(bb.log.last(), Some(&Task::ShootTarget));
    }

    #[test]
    fn wait_and_cooldown() {
        let t = Bt::selector([
            Bt::task(Task::Bark(Bark::Searching)).cooldown(5.0),
            Bt::wait(1.0),
        ])
        .build();
        let mut mem = vec![NodeMem::default(); t.len()];
        let mut bb = TestBb::default();
        assert_eq!(t.tick(&mut bb, &mut mem, 1), Status::Success); // bark
        bb.time = 0.5;
        assert_eq!(t.tick(&mut bb, &mut mem, 2), Status::Running); // cooling down -> wait
        bb.time = 1.6;
        assert_eq!(t.tick(&mut bb, &mut mem, 3), Status::Success); // wait done
        bb.time = 5.1;
        t.tick(&mut bb, &mut mem, 4);
        assert_eq!(
            bb.log
                .iter()
                .filter(|t| **t == Task::Bark(Bark::Searching))
                .count(),
            2
        );
    }
}
