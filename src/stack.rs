use serde::{Deserialize, Serialize};

use crate::rule::BsRule;

/// One segment of a rule stack: run `rule` for `generations` steps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StackEntry {
    pub name: String,
    pub rule: BsRule,
    pub generations: u32,
}

/// A looping sequence of rules. Plain, serializable data — no UI state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleStack {
    pub entries: Vec<StackEntry>,
}

impl RuleStack {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn total_generations(&self) -> u64 {
        self.entries.iter().map(|e| e.generations as u64).sum()
    }

    /// Which entry is active at the given generation. The stack loops: after
    /// the last segment finishes, the first starts again.
    pub fn entry_at(&self, generation: u64) -> Option<(usize, &StackEntry)> {
        let total = self.total_generations();
        if total == 0 {
            return None;
        }
        let mut offset = generation % total;
        for (index, entry) in self.entries.iter().enumerate() {
            if offset < entry.generations as u64 {
                return Some((index, entry));
            }
            offset -= entry.generations as u64;
        }
        unreachable!("offset < total ensures a segment matches")
    }
}

/// An undoable edit to the rule stack. Each variant records what it needs to
/// invert itself: add↔remove, move↔move-back, set stores the old value.
#[derive(Clone, Debug)]
pub enum EditCommand {
    Add { index: usize, entry: StackEntry },
    Remove { index: usize, entry: StackEntry },
    SetGenerations { index: usize, old: u32, new: u32 },
    Move { from: usize, to: usize },
}

impl EditCommand {
    fn apply_to(&self, stack: &mut RuleStack) {
        match self {
            EditCommand::Add { index, entry } => stack.entries.insert(*index, entry.clone()),
            EditCommand::Remove { index, .. } => {
                stack.entries.remove(*index);
            }
            EditCommand::SetGenerations { index, new, .. } => {
                stack.entries[*index].generations = *new;
            }
            EditCommand::Move { from, to } => {
                let entry = stack.entries.remove(*from);
                stack.entries.insert(*to, entry);
            }
        }
    }

    fn inverted(&self) -> EditCommand {
        match self.clone() {
            EditCommand::Add { index, entry } => EditCommand::Remove { index, entry },
            EditCommand::Remove { index, entry } => EditCommand::Add { index, entry },
            EditCommand::SetGenerations { index, old, new } => EditCommand::SetGenerations {
                index,
                old: new,
                new: old,
            },
            EditCommand::Move { from, to } => EditCommand::Move { from: to, to: from },
        }
    }
}

/// The rule stack plus its edit history.
#[derive(Default)]
pub struct StackEditor {
    pub stack: RuleStack,
    undo: Vec<EditCommand>,
    redo: Vec<EditCommand>,
}

impl StackEditor {
    pub fn apply(&mut self, command: EditCommand) {
        command.apply_to(&mut self.stack);
        self.undo.push(command);
        self.redo.clear();
    }

    pub fn undo(&mut self) -> bool {
        let Some(command) = self.undo.pop() else {
            return false;
        };
        let inverse = command.inverted();
        inverse.apply_to(&mut self.stack);
        self.redo.push(command);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(command) = self.redo.pop() else {
            return false;
        };
        command.apply_to(&mut self.stack);
        self.undo.push(command);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, notation: &str, generations: u32) -> StackEntry {
        StackEntry {
            name: name.to_string(),
            rule: BsRule::parse(notation).unwrap(),
            generations,
        }
    }

    fn editor_with(entries: &[(&str, &str, u32)]) -> StackEditor {
        let mut editor = StackEditor::default();
        for (i, &(name, notation, generations)) in entries.iter().enumerate() {
            editor.apply(EditCommand::Add {
                index: i,
                entry: entry(name, notation, generations),
            });
        }
        editor
    }

    #[test]
    fn entry_at_walks_segments_and_loops() {
        let editor = editor_with(&[("Life", "B3/S23", 10), ("Seeds", "B2/S", 5)]);
        let stack = &editor.stack;
        assert_eq!(stack.entry_at(0).unwrap().0, 0);
        assert_eq!(stack.entry_at(9).unwrap().0, 0);
        assert_eq!(stack.entry_at(10).unwrap().0, 1);
        assert_eq!(stack.entry_at(14).unwrap().0, 1);
        assert_eq!(stack.entry_at(15).unwrap().0, 0); // wrapped
        assert_eq!(stack.entry_at(40).unwrap().0, 1);
    }

    #[test]
    fn empty_stack_has_no_active_entry() {
        assert_eq!(RuleStack::default().entry_at(0), None);
    }

    #[test]
    fn undo_redo_round_trips_all_commands() {
        let mut editor = editor_with(&[("Life", "B3/S23", 10), ("Seeds", "B2/S", 5)]);
        editor.apply(EditCommand::SetGenerations {
            index: 1,
            old: 5,
            new: 50,
        });
        editor.apply(EditCommand::Move { from: 1, to: 0 });
        editor.apply(EditCommand::Remove {
            index: 1,
            entry: editor.stack.entries[1].clone(),
        });
        let final_state = editor.stack.clone();
        assert_eq!(final_state.entries.len(), 1);
        assert_eq!(final_state.entries[0].name, "Seeds");
        assert_eq!(final_state.entries[0].generations, 50);

        while editor.undo() {}
        assert!(editor.stack.is_empty());

        while editor.redo() {}
        assert_eq!(editor.stack, final_state);
    }

    #[test]
    fn new_edit_clears_redo_history() {
        let mut editor = editor_with(&[("Life", "B3/S23", 10)]);
        editor.undo();
        assert!(editor.can_redo());
        editor.apply(EditCommand::Add {
            index: 0,
            entry: entry("Seeds", "B2/S", 5),
        });
        assert!(!editor.can_redo());
    }

    #[test]
    fn stack_serializes_to_json_and_back() {
        let stack = editor_with(&[("Life", "B3/S23", 100), ("Day & Night", "B3678/S34678", 25)])
            .stack
            .clone();
        let json = serde_json::to_string(&stack).unwrap();
        assert!(
            json.contains("B3/S23"),
            "rules serialize as notation: {json}"
        );
        let back: RuleStack = serde_json::from_str(&json).unwrap();
        assert_eq!(back, stack);
    }
}
