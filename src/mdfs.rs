use std::collections::hash_map::Entry::{Occupied, Vacant};
use std::collections::HashMap;
use std::mem;

use roaring::RoaringBitmap;
use crate::Index;

/// A mana depth first search implementation.
pub struct Mdfs<'a> {
    index: &'a Index,
    rtxn: &'a heed::RoTxn,
    words: &'a [(HashMap<String, (u8, RoaringBitmap)>, RoaringBitmap)],
    union_cache: HashMap<(usize, u8), RoaringBitmap>,
    candidates: RoaringBitmap,
    mana: u32,
    max_mana: u32,
}

impl<'a> Mdfs<'a> {
    pub fn new(
        index: &'a Index,
        rtxn: &'a heed::RoTxn,
        words: &'a [(HashMap<String, (u8, RoaringBitmap)>, RoaringBitmap)],
        candidates: RoaringBitmap,
    ) -> Mdfs<'a>
    {
        // Compute the number of pairs (windows) we have for this list of words.
        let mana = words.len().checked_sub(1).unwrap_or(0) as u32;
        let max_mana = mana * 8;
        Mdfs { index, rtxn, words, union_cache: HashMap::new(), candidates, mana, max_mana }
    }
}

impl<'a> Iterator for Mdfs<'a> {
    type Item = anyhow::Result<RoaringBitmap>;

    fn next(&mut self) -> Option<Self::Item> {
        // If there is less or only one word therefore the only
        // possible documents that we can return are the candidates.
        if self.words.len() <= 1 {
            if self.candidates.is_empty() { return None }
            return Some(Ok(mem::take(&mut self.candidates)));
        }

        let mut answer = RoaringBitmap::new();
        while self.mana <= self.max_mana {
            let result = mdfs_step(
                &self.index,
                &self.rtxn,
                self.mana,
                self.words,
                &self.candidates,
                &self.candidates,
                &mut self.union_cache,
            );

            match result {
                Ok(Some(a)) => {
                    // We remove the answered documents from the list of
                    // candidates to be sure we don't search for them again.
                    self.candidates.difference_with(&a);
                    answer.union_with(&a);
                },
                Ok(None) => {
                    // We found the last iteration for this amount of mana that gives nothing,
                    // we can now store that the next mana to use for the loop is incremented.
                    self.mana = self.mana + 1;
                    // If the answer is empty it means that we found nothing for this amount
                    // of mana therefore we continue with a bigger mana.
                    if !answer.is_empty() {
                        // Otherwise we return the answer.
                        return Some(Ok(answer));
                    }
                },
                Err(e) => return Some(Err(e)),
            }
        }

        None
    }
}

fn mdfs_step(
    index: &Index,
    rtxn: &heed::RoTxn,
    mana: u32,
    words: &[(HashMap<String, (u8, RoaringBitmap)>, RoaringBitmap)],
    candidates: &RoaringBitmap,
    parent_docids: &RoaringBitmap,
    union_cache: &mut HashMap<(usize, u8), RoaringBitmap>,
) -> anyhow::Result<Option<RoaringBitmap>>
{
    use std::cmp::{min, max};

    let (words1, words2) = (&words[0].0, &words[1].0);
    let pairs = words_pair_combinations(words1, words2);
    let tail = &words[1..];
    let nb_children = tail.len() as u32 - 1;

    // The minimum amount of mana that you must consume is at least 1 and the
    // amount of mana that your children can consume. Because the last child must
    // consume the remaining mana, it is mandatory that there not too much at the end.
    let min_proximity = max(1, mana.saturating_sub(nb_children * 8)) as u8;

    // The maximum amount of mana that you can use is 8 or the remaining amount of
    // mana minus your children, as you can't just consume all the mana,
    // your children must have at least 1 mana.
    let max_proximity = min(8, mana - nb_children) as u8;

    for proximity in min_proximity..=max_proximity {
        let mut docids = match union_cache.entry((words.len(), proximity)) {
            Occupied(entry) => entry.get().clone(),
            Vacant(entry) => {
                let mut docids = RoaringBitmap::new();
                if proximity == 8 {
                    docids = candidates.clone();
                } else {
                    for (w1, w2) in pairs.iter().cloned() {
                        let key = (w1, w2, proximity);
                        if let Some(di) = index.word_pair_proximity_docids.get(rtxn, &key)? {
                            docids.union_with(&di);
                        }
                    }
                }
                entry.insert(docids).clone()
            }
        };

        docids.intersect_with(parent_docids);

        if !docids.is_empty() {
            let mana = mana.checked_sub(proximity as u32).unwrap();
            // We are the last pair, we return without recursing as we don't have any child.
            if tail.len() < 2 { return Ok(Some(docids)) }
            if let Some(di) = mdfs_step(index, rtxn, mana, tail, candidates, &docids, union_cache)? {
                return Ok(Some(di))
            }
        }
    }

    Ok(None)
}

fn words_pair_combinations<'h>(
    w1: &'h HashMap<String, (u8, RoaringBitmap)>,
    w2: &'h HashMap<String, (u8, RoaringBitmap)>,
) -> Vec<(&'h str, &'h str)>
{
    let mut pairs = Vec::new();
    for (w1, (_typos, docids1)) in w1 {
        for (w2, (_typos, docids2)) in w2 {
            if !docids1.is_disjoint(&docids2) {
                pairs.push((w1.as_str(), w2.as_str()));
            }
        }
    }
    pairs
}
