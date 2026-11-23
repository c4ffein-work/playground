//! Merging two lines of a store, as `apps/pass/SYNC.md` says ("Taking a device's update"):
//! entries are whole files, followed through moves; what both sides did differently is a
//! choice, mine or theirs, never a mix.
//!
//! Nothing here reads a repository or a key: versions are opaque ids, and whether two of
//! them are "the same" (equal bytes, or equal once decrypted) is asked of the caller.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use gix::ObjectId;

use crate::Side;

/// The entries of one commit: name (`Email/work`) → version.
pub(crate) type Entries = BTreeMap<String, ObjectId>;

/// Where an entry is on one side, and what it holds there. `None`: it is not there.
type Place = Option<(String, ObjectId)>;

/// One entry, followed through moves: as it was, and as each side has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) base: Place,
    pub(crate) mine: Place,
    pub(crate) theirs: Place,
}

/// The moves one side made: an entry gone from one name and one come at another with the
/// same content. One to one, in name order. Old name → new name.
pub(crate) fn moves(
    base: &Entries,
    side: &Entries,
    same: &mut dyn FnMut(ObjectId, ObjectId) -> bool,
) -> BTreeMap<String, String> {
    let added: Vec<&String> = side.keys().filter(|name| !base.contains_key(*name)).collect();
    let mut taken = BTreeSet::new();
    let mut moved = BTreeMap::new();
    for (name, version) in base.iter().filter(|(name, _)| !side.contains_key(*name)) {
        let target = added
            .iter()
            .find(|target| !taken.contains(**target) && same(*version, side[**target]));
        if let Some(target) = target {
            taken.insert((*target).clone());
            moved.insert(name.clone(), (*target).clone());
        }
    }
    moved
}

/// Every entry of the three sides, each followed through the moves of both.
pub(crate) fn items(
    base: &Entries,
    mine: &Entries,
    theirs: &Entries,
    same: &mut dyn FnMut(ObjectId, ObjectId) -> bool,
) -> Vec<Item> {
    let (my_moves, their_moves) = (moves(base, mine, same), moves(base, theirs, same));
    let place = |side: &Entries, moved: &BTreeMap<String, String>, name: &String| -> Place {
        let now = if side.contains_key(name) {
            Some(name)
        } else {
            moved.get(name)
        };
        now.map(|now| (now.clone(), side[now]))
    };
    let mut items: Vec<Item> = base
        .iter()
        .map(|(name, version)| Item {
            base: Some((name.clone(), *version)),
            mine: place(mine, &my_moves, name),
            theirs: place(theirs, &their_moves, name),
        })
        .collect();
    // What is new on a side: there, not in the base, and not where a moved entry went.
    let new = |side: &Entries, moved: &BTreeMap<String, String>| -> BTreeSet<String> {
        let targets: BTreeSet<&String> = moved.values().collect();
        side.keys()
            .filter(|name| !base.contains_key(*name) && !targets.contains(name))
            .cloned()
            .collect()
    };
    let (my_new, their_new) = (new(mine, &my_moves), new(theirs, &their_moves));
    for name in my_new.union(&their_new) {
        let on = |side: &Entries, new: &BTreeSet<String>| new.contains(name).then(|| (name.clone(), side[name]));
        items.push(Item {
            base: None,
            mine: on(mine, &my_new),
            theirs: on(theirs, &their_new),
        });
    }
    items
}

/// What one line of the table of SYNC.md gives for an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// Where the entry ends up, and with what. `None`: it is gone.
    Result(Place),
    /// Both sides did something, and not the same thing: a choice, by this name.
    Choice(String),
}

/// The name a choice goes by: the entry's on this device's side, or on theirs if it is gone here.
fn choice_name(item: &Item) -> String {
    let place = item.mine.as_ref().or(item.theirs.as_ref()).or(item.base.as_ref());
    place.map(|(name, _)| name.clone()).unwrap_or_default()
}

fn verdict(item: &Item, same: &mut dyn FnMut(ObjectId, ObjectId) -> bool) -> Verdict {
    let Some((base_name, base_version)) = &item.base else {
        // New on one side, or on both.
        return match (&item.mine, &item.theirs) {
            (Some(mine), Some(theirs)) if !same(mine.1, theirs.1) => Verdict::Choice(mine.0.clone()),
            (Some(mine), _) => Verdict::Result(Some(mine.clone())),
            (None, theirs) => Verdict::Result(theirs.clone()),
        };
    };
    // What a side did to the entry: moved it (its name), changed it (its content), or neither.
    let did = |place: &Place, same: &mut dyn FnMut(ObjectId, ObjectId) -> bool| {
        place
            .as_ref()
            .map(|(name, version)| (name != base_name, !same(*base_version, *version)))
    };
    match (&item.mine, &item.theirs) {
        (None, None) => Verdict::Result(None),
        // A removal stands when the other side did nothing; against any change it is a choice.
        (None, Some(_)) => match did(&item.theirs, same) {
            Some((false, false)) => Verdict::Result(None),
            _ => Verdict::Choice(choice_name(item)),
        },
        (Some(_), None) => match did(&item.mine, same) {
            Some((false, false)) => Verdict::Result(None),
            _ => Verdict::Choice(choice_name(item)),
        },
        (Some(mine), Some(theirs)) => {
            let (my_move, my_change) = did(&item.mine, same).expect("it is there");
            let (their_move, their_change) = did(&item.theirs, same).expect("it is there");
            if (my_move && their_move && mine.0 != theirs.0) || (my_change && their_change && !same(mine.1, theirs.1)) {
                return Verdict::Choice(mine.0.clone());
            }
            let name = if my_move { &mine.0 } else { &theirs.0 };
            // The content of whoever changed it. When nobody did, a version that was only encrypted
            // again counts (it is what makes an entry readable by its new readers): theirs if
            // only theirs was, mine otherwise.
            let version = if my_change {
                mine.1
            } else if their_change || mine.1 == *base_version {
                theirs.1
            } else {
                mine.1
            };
            Verdict::Result(Some((name.clone(), version)))
        }
    }
}

/// What merging gives: the entries, or the choices it still needs.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Merged {
    pub(crate) entries: Entries,
    /// The names of the choices that were needed and not given, sorted. Not empty: nothing is to be taken.
    pub(crate) undecided: Vec<String>,
    /// Every choice the merge needs, given or not, sorted: what a review shows as conflicts.
    pub(crate) conflicts: Vec<String>,
}

/// Merges the entries of two sides that started from `base`.
pub(crate) fn merge(
    base: &Entries,
    mine: &Entries,
    theirs: &Entries,
    choices: &HashMap<String, Side>,
    same: &mut dyn FnMut(ObjectId, ObjectId) -> bool,
) -> Merged {
    let items = items(base, mine, theirs, same);
    let mut merged = Merged::default();
    let mut conflicts = BTreeSet::new();
    let mut undecided = BTreeSet::new();
    // Each entry on its own first: where it lands, and with what.
    let mut placed: Vec<(usize, Place)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match verdict(item, same) {
            Verdict::Result(place) => placed.push((index, place)),
            Verdict::Choice(name) => {
                conflicts.insert(name.clone());
                match choices.get(&name) {
                    Some(Side::Mine) => placed.push((index, item.mine.clone())),
                    Some(Side::Theirs) => placed.push((index, item.theirs.clone())),
                    None => {
                        undecided.insert(name);
                    }
                }
            }
        }
    }
    // Then together: two entries cannot end under one name (one moved there, another was added
    // there). When they do not hold the same thing, that too is a choice, by that name, and
    // whole: each of the entries involved as this side has it, or each as theirs has it.
    let colliding = |placed: &[(usize, Place)], same: &mut dyn FnMut(ObjectId, ObjectId) -> bool| {
        let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (position, (_, place)) in placed.iter().enumerate() {
            if let Some((name, _)) = place {
                by_name.entry(name.clone()).or_default().push(position);
            }
        }
        by_name.retain(|_, positions| {
            let version = |position: usize| placed[position].1.as_ref().expect("it is placed").1;
            let first = version(positions[0]);
            positions.iter().any(|position| !same(first, version(*position)))
        });
        by_name
    };
    for (name, positions) in colliding(&placed, same) {
        conflicts.insert(name.clone());
        match choices.get(&name) {
            Some(side) => {
                for position in positions {
                    let item = &items[placed[position].0];
                    placed[position].1 = match side {
                        Side::Mine => item.mine.clone(),
                        Side::Theirs => item.theirs.clone(),
                    };
                }
            }
            None => {
                undecided.insert(name);
            }
        }
    }
    // Two entries still under one name after that: the choices given cannot all hold.
    let still: BTreeSet<String> = colliding(&placed, same).into_keys().collect();
    for (_, place) in placed {
        if let Some((name, version)) = place {
            merged.entries.entry(name).or_insert(version);
        }
    }
    for name in still {
        if undecided.insert(name.clone()) {
            conflicts.insert(name);
        }
    }
    merged.undecided = undecided.into_iter().collect();
    merged.conflicts = conflicts.into_iter().collect();
    merged
}

/// What one side did since the base, as a review tells it. Sorted by name within each kind.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    pub(crate) added: Vec<String>,
    pub(crate) changed: Vec<String>,
    /// Old name, new name.
    pub(crate) moved: Vec<(String, String)>,
    pub(crate) removed: Vec<String>,
}

pub(crate) fn changes(base: &Entries, side: &Entries, same: &mut dyn FnMut(ObjectId, ObjectId) -> bool) -> Changes {
    let moved = moves(base, side, same);
    let targets: BTreeSet<&String> = moved.values().collect();
    let mut changes = Changes::default();
    for (name, version) in base {
        match side.get(name) {
            Some(now) if !same(*version, *now) => changes.changed.push(name.clone()),
            Some(_) => {}
            None if moved.contains_key(name) => {}
            None => changes.removed.push(name.clone()),
        }
    }
    for name in side.keys() {
        if !base.contains_key(name) && !targets.contains(name) {
            changes.added.push(name.clone());
        }
    }
    changes.moved = moved.into_iter().collect();
    changes
}

/// A file that is merged whole and by its bytes (a readers file): what the two sides make of it,
/// or `Err` when both changed it differently and no choice was given.
pub(crate) fn merge_whole(
    base: Option<ObjectId>,
    mine: Option<ObjectId>,
    theirs: Option<ObjectId>,
    choice: Option<Side>,
) -> Result<Option<ObjectId>, ()> {
    if mine == theirs || theirs == base {
        Ok(mine)
    } else if mine == base {
        Ok(theirs)
    } else {
        match choice {
            Some(Side::Mine) => Ok(mine),
            Some(Side::Theirs) => Ok(theirs),
            None => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version: `content` is what it decrypts to, `copy` tells two encryptions of it apart.
    fn version(content: u8, copy: u8) -> ObjectId {
        ObjectId::from_hex(format!("{content:02x}{copy:02x}{:036x}", 0).as_bytes()).unwrap()
    }

    fn v(content: u8) -> ObjectId {
        version(content, 0)
    }

    /// Two versions are the same when they hold the same content, however encrypted.
    fn same(a: ObjectId, b: ObjectId) -> bool {
        a.as_bytes()[0] == b.as_bytes()[0]
    }

    fn entries(list: &[(&str, ObjectId)]) -> Entries {
        list.iter()
            .map(|(name, version)| (name.to_string(), *version))
            .collect()
    }

    fn run(base: &Entries, mine: &Entries, theirs: &Entries, choices: &[(&str, Side)]) -> Merged {
        let choices = choices.iter().map(|(name, side)| (name.to_string(), *side)).collect();
        merge(base, mine, theirs, &choices, &mut same)
    }

    /// Merged with no choice needed, both ways round.
    fn settles(base: &Entries, a: &Entries, b: &Entries, expected: &[(&str, ObjectId)]) {
        for (mine, theirs) in [(a, b), (b, a)] {
            let merged = run(base, mine, theirs, &[]);
            assert_eq!(merged.conflicts, Vec::<String>::new());
            let got: Vec<(&str, u8)> = merged
                .entries
                .iter()
                .map(|(name, version)| (name.as_str(), version.as_bytes()[0]))
                .collect();
            let want: Vec<(&str, u8)> = expected
                .iter()
                .map(|(name, version)| (*name, version.as_bytes()[0]))
                .collect();
            assert_eq!(got, want);
        }
    }

    #[test]
    fn one_side_only() {
        let base = entries(&[("bank", v(1)), ("wifi", v(2)), ("old", v(3))]);
        // Changed, moved, removed, added: against a side that did nothing, the change.
        let changed = entries(&[("bank", v(9)), ("home/wifi", v(2)), ("new", v(4))]);
        settles(
            &base,
            &changed,
            &base,
            &[("bank", v(9)), ("home/wifi", v(2)), ("new", v(4))],
        );
        // Different entries on each side.
        let other = entries(&[("bank", v(1)), ("wifi", v(8)), ("old", v(3)), ("theirs", v(5))]);
        settles(
            &base,
            &changed,
            &other,
            // (scenario 07) moved on one side, changed on the other: the new content under the new name.
            &[("bank", v(9)), ("home/wifi", v(8)), ("new", v(4)), ("theirs", v(5))],
        );
    }

    #[test]
    fn a_folder_moved_while_the_other_side_works_in_it() {
        // Scenario 08.
        let base = entries(&[("old/a", v(1)), ("old/b", v(2)), ("old/c", v(3))]);
        let moved = entries(&[("new/a", v(1)), ("new/b", v(2)), ("new/c", v(3))]);
        let worked = entries(&[("old/a", v(1)), ("old/b", v(7)), ("old/c", v(3)), ("old/d", v(4))]);
        settles(
            &base,
            &moved,
            &worked,
            &[("new/a", v(1)), ("new/b", v(7)), ("new/c", v(3)), ("old/d", v(4))],
        );
    }

    #[test]
    fn the_same_thing_done_on_both_sides() {
        let base = entries(&[("bank", v(1)), ("wifi", v(2)), ("old", v(3))]);
        // The same new content typed on each (two encryptions of it), the same move, the same removal,
        // the same new entry (scenario 10).
        let a = entries(&[("bank", version(9, 1)), ("home/wifi", v(2)), ("new", version(4, 1))]);
        let b = entries(&[("bank", version(9, 2)), ("home/wifi", v(2)), ("new", version(4, 2))]);
        settles(&base, &a, &b, &[("bank", v(9)), ("home/wifi", v(2)), ("new", v(4))]);
        // Each keeps its own bytes of what it already had.
        assert_eq!(run(&base, &a, &b, &[]).entries["bank"], version(9, 1));
        assert_eq!(run(&base, &b, &a, &[]).entries["bank"], version(9, 2));
    }

    #[test]
    fn an_entry_only_encrypted_again_is_not_a_change_of_content() {
        let base = entries(&[("bank", version(1, 0))]);
        let again = entries(&[("bank", version(1, 1))]);
        // Against nothing: the new encryption is taken (it is what the new readers can read).
        assert_eq!(run(&base, &base, &again, &[]).entries["bank"], version(1, 1));
        assert_eq!(run(&base, &again, &base, &[]).entries["bank"], version(1, 1));
        // Both did it on their own (scenario 29): no conflict, each keeps its own.
        let also = entries(&[("bank", version(1, 2))]);
        let merged = run(&base, &again, &also, &[]);
        assert_eq!((merged.conflicts.len(), merged.entries["bank"]), (0, version(1, 1)));
        // Against a change of content: the content wins, the re-encryption is to be done again.
        let changed = entries(&[("bank", version(5, 0))]);
        settles(&base, &again, &changed, &[("bank", v(5))]);
        // It does not show as a change, nor hide a move.
        assert_eq!(changes(&base, &again, &mut same), Changes::default());
        let moved = entries(&[("money", version(1, 3))]);
        assert_eq!(
            changes(&base, &moved, &mut same).moved,
            [("bank".to_string(), "money".to_string())]
        );
    }

    #[test]
    fn what_needs_a_choice() {
        let base = entries(&[("bank", v(1)), ("wifi", v(2))]);
        // Two contents (scenarios 04, 05).
        let (a, b) = (
            entries(&[("bank", v(8)), ("wifi", v(2))]),
            entries(&[("bank", v(9)), ("wifi", v(2))]),
        );
        let undecided = run(&base, &a, &b, &[]);
        assert_eq!(
            (undecided.conflicts.clone(), undecided.undecided),
            (vec!["bank".to_string()], vec!["bank".to_string()])
        );
        assert_eq!(run(&base, &a, &b, &[("bank", Side::Mine)]).entries["bank"], v(8));
        assert_eq!(run(&base, &a, &b, &[("bank", Side::Theirs)]).entries["bank"], v(9));
        assert!(run(&base, &a, &b, &[("bank", Side::Mine)]).undecided.is_empty());
        // A choice for something else decides nothing.
        assert_eq!(run(&base, &a, &b, &[("wifi", Side::Mine)]).undecided, ["bank"]);

        // A removal against a change (scenario 06): named by where it still is.
        let removed = entries(&[("wifi", v(2))]);
        for (mine, theirs, mine_keeps) in [(&removed, &b, false), (&b, &removed, true)] {
            assert_eq!(run(&base, mine, theirs, &[]).conflicts, ["bank"]);
            let kept_mine = run(&base, mine, theirs, &[("bank", Side::Mine)]).entries;
            assert_eq!(kept_mine.contains_key("bank"), mine_keeps);
            let kept_theirs = run(&base, mine, theirs, &[("bank", Side::Theirs)]).entries;
            assert_eq!(kept_theirs.contains_key("bank"), !mine_keeps);
        }
        // A removal against a move: named by the new name, the only one left.
        let moved = entries(&[("money", v(1)), ("wifi", v(2))]);
        assert_eq!(run(&base, &removed, &moved, &[]).conflicts, ["money"]);
        assert_eq!(run(&base, &moved, &removed, &[]).conflicts, ["money"]);

        // Two names (scenario 09): named by this side's, and the chosen name only.
        let elsewhere = entries(&[("cash", v(1)), ("wifi", v(2))]);
        assert_eq!(run(&base, &moved, &elsewhere, &[]).conflicts, ["money"]);
        assert_eq!(run(&base, &elsewhere, &moved, &[]).conflicts, ["cash"]);
        let mine: Vec<String> = run(&base, &moved, &elsewhere, &[("money", Side::Mine)])
            .entries
            .into_keys()
            .collect();
        assert_eq!(mine, ["money", "wifi"]);
        let theirs: Vec<String> = run(&base, &moved, &elsewhere, &[("money", Side::Theirs)])
            .entries
            .into_keys()
            .collect();
        assert_eq!(theirs, ["cash", "wifi"]);

        // Two new entries of one name (scenario 11).
        let (a, b) = (
            entries(&[("bank", v(1)), ("wifi", v(2)), ("new", v(5))]),
            entries(&[("bank", v(1)), ("wifi", v(2)), ("new", v(6))]),
        );
        assert_eq!(run(&base, &a, &b, &[]).conflicts, ["new"]);
        assert_eq!(run(&base, &a, &b, &[("new", Side::Theirs)]).entries["new"], v(6));
    }

    #[test]
    fn two_entries_cannot_end_under_one_name() {
        // Scenario 12: one side moves x to y, the other adds a new y.
        let base = entries(&[("x", v(1))]);
        let moved = entries(&[("y", v(1))]);
        let added = entries(&[("x", v(1)), ("y", v(2))]);
        for (mine, theirs) in [(&moved, &added), (&added, &moved)] {
            let merged = run(&base, mine, theirs, &[]);
            assert_eq!(merged.conflicts, ["y"]);
            assert_eq!(merged.undecided, ["y"]);
            // Whole: each side's choice is that side's state of both entries involved.
            assert_eq!(run(&base, mine, theirs, &[("y", Side::Mine)]).entries, *mine);
            assert_eq!(run(&base, mine, theirs, &[("y", Side::Theirs)]).entries, *theirs);
        }
    }

    #[test]
    fn two_entries_with_the_same_content_can_end_under_one_name() {
        // One side moves x to y, the other adds a y that says the same: one y, no choice.
        let base = entries(&[("x", v(1))]);
        let moved = entries(&[("y", version(1, 1))]);
        let added = entries(&[("x", v(1)), ("y", version(1, 2))]);
        settles(&base, &moved, &added, &[("y", v(1))]);
    }

    #[test]
    fn moves_are_matched_one_to_one_in_name_order() {
        // Two entries with the same content go, two come: the first to the first.
        let base = entries(&[("a", v(1)), ("b", v(1)), ("c", v(2))]);
        let side = entries(&[("y", v(1)), ("z", v(1)), ("c", v(2))]);
        let moved = moves(&base, &side, &mut same);
        assert_eq!(
            moved.into_iter().collect::<Vec<_>>(),
            [("a".to_string(), "y".to_string()), ("b".to_string(), "z".to_string())]
        );
        // Moved and changed at once is not recognised: a removal and an addition.
        let side = entries(&[("a", v(1)), ("b", v(1)), ("d", v(3))]);
        let told = changes(&base, &side, &mut same);
        assert_eq!(
            (told.removed, told.added, told.moved.len()),
            (vec!["c".to_string()], vec!["d".to_string()], 0)
        );
        // One gone and two candidates: one move, one addition.
        let side = entries(&[("b", v(1)), ("c", v(2)), ("m", v(1)), ("n", v(1))]);
        let told = changes(&base, &side, &mut same);
        assert_eq!(told.moved, [("a".to_string(), "m".to_string())]);
        assert_eq!(told.added, ["n"]);
    }

    #[test]
    fn readers_files_are_merged_whole() {
        let (base, a, b) = (Some(v(1)), Some(v(2)), Some(v(3)));
        assert_eq!(merge_whole(base, a, base, None), Ok(a));
        assert_eq!(merge_whole(base, base, b, None), Ok(b));
        assert_eq!(merge_whole(base, a, a, None), Ok(a));
        assert_eq!(merge_whole(base, None, base, None), Ok(None), "removed on one side");
        assert_eq!(merge_whole(None, a, None, None), Ok(a), "added on one side");
        assert_eq!(merge_whole(base, a, b, None), Err(()));
        assert_eq!(merge_whole(base, a, None, None), Err(()), "changed here, removed there");
        assert_eq!(merge_whole(base, a, b, Some(Side::Mine)), Ok(a));
        assert_eq!(merge_whole(base, a, b, Some(Side::Theirs)), Ok(b));
    }
}
