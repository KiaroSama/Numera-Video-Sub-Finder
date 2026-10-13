use crate::{Cue, Error, Result};

#[derive(Default)]
pub struct EditHistory {
    undo: Vec<Vec<Cue>>,
    redo: Vec<Vec<Cue>>,
}

fn snapshot_bytes(cues: &[Cue]) -> usize {
    cues.iter()
        .map(|c| {
            std::mem::size_of::<Cue>()
                + c.text.len()
                + c.raw_text.len()
                + c.crop.as_ref().map_or(0, String::len)
        })
        .sum()
}

impl EditHistory {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn apply(
        &mut self,
        cues: &mut Vec<Cue>,
        change: impl FnOnce(&mut Vec<Cue>) -> Result<()>,
    ) -> Result<()> {
        let before = cues.clone();
        if let Err(e) = change(cues).and_then(|_| cues.iter().try_for_each(Cue::validate)) {
            *cues = before;
            return Err(e);
        }
        self.undo.push(before);
        while self.undo.len() > 1
            && (self.undo.len() > 50
                || self.undo.iter().map(|v| snapshot_bytes(v)).sum::<usize>() > 32 * 1024 * 1024)
        {
            self.undo.remove(0);
        }
        self.redo.clear();
        Ok(())
    }
    pub fn undo(&mut self, cues: &mut Vec<Cue>) -> bool {
        if let Some(previous) = self.undo.pop() {
            self.redo.push(std::mem::replace(cues, previous));
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self, cues: &mut Vec<Cue>) -> bool {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(cues, next));
            true
        } else {
            false
        }
    }
}

pub fn split_cue(cues: &mut Vec<Cue>, id: u64, at_ms: u64) -> Result<()> {
    let i = cues
        .iter()
        .position(|c| c.id == id)
        .ok_or_else(|| Error::Invalid("cue not found".into()))?;
    let mut second = cues[i].clone();
    if at_ms <= second.start_ms || at_ms >= second.end_ms {
        return Err(Error::Invalid("split must be inside cue".into()));
    }
    second.id = cues
        .iter()
        .map(|c| c.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(Error::Overflow)?;
    second.start_ms = at_ms;
    second.edited = true;
    cues[i].end_ms = at_ms;
    cues[i].edited = true;
    cues.insert(i + 1, second);
    Ok(())
}

pub fn merge_cues(cues: &mut Vec<Cue>, first: u64, second: u64) -> Result<()> {
    let a = cues
        .iter()
        .position(|c| c.id == first)
        .ok_or_else(|| Error::Invalid("cue not found".into()))?;
    let b = cues
        .iter()
        .position(|c| c.id == second)
        .ok_or_else(|| Error::Invalid("cue not found".into()))?;
    if a == b || cues[a].track != cues[b].track {
        return Err(Error::Invalid(
            "merge requires distinct cues in the same track".into(),
        ));
    }
    let later = cues[b].clone();
    let mut merged = cues[a].clone();
    merged.start_ms = merged.start_ms.min(later.start_ms);
    merged.end_ms = merged.end_ms.max(later.end_ms);
    merged.text = format!("{}\n{}", merged.text, later.text);
    merged.edited = true;
    merged.validate()?;
    cues[a] = merged;
    cues.remove(b);
    Ok(())
}
