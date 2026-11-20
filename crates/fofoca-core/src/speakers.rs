//! Putting speaker turns (spec 003) onto the transcript.
//!
//! Only `Call` segments are re-attributed — the mic track already says it's **you**. Each word
//! takes the turn it overlaps most; a segment whose words change speaker is split there.

use std::collections::BTreeMap;

use crate::diarize::Turn;
use crate::model::{Segment, SpeakerInfo, Track, Word};

/// Speaker key for the local user (mic track).
pub const YOU: &str = "you";

pub fn key(speaker: u32) -> String {
    format!("s{speaker}")
}

/// Fewer words than this are folded into the neighbouring piece instead of becoming their own line.
const MIN_PIECE_WORDS: usize = 2;

pub fn assign(segments: &[Segment], turns: &[Turn]) -> Vec<Segment> {
    let mut out = Vec::with_capacity(segments.len());
    for seg in segments {
        if seg.track == Track::You {
            out.push(Segment { speaker: Some(YOU.into()), ..seg.clone() });
            continue;
        }
        if seg.words.is_empty() {
            out.push(Segment { speaker: best_turn(turns, seg.start, seg.end).map(key), ..seg.clone() });
            continue;
        }
        let mut labels: Vec<Option<u32>> = seg.words.iter().map(|w| best_turn(turns, w.start, w.end)).collect();
        fill_gaps(&mut labels);
        for (speaker, words) in pieces(&seg.words, &labels) {
            out.push(Segment {
                start: words.first().map(|w| w.start).unwrap_or(seg.start),
                end: words.last().map(|w| w.end).unwrap_or(seg.end),
                // words carry Whisper's leading space (" dois")
                text: words.iter().map(|w| w.text.as_str()).collect::<String>().trim().to_string(),
                track: seg.track,
                words: words.to_vec(),
                speaker: speaker.map(key),
            });
        }
    }
    out
}

/// Turn with the largest overlap with `[a, b]`; if none overlaps, the closest one within 1 s.
fn best_turn(turns: &[Turn], a: f64, b: f64) -> Option<u32> {
    let overlap = |t: &Turn| (b.min(t.end) - a.max(t.start)).max(0.0);
    if let Some(t) = turns.iter().filter(|t| overlap(t) > 0.0).max_by(|x, y| overlap(x).total_cmp(&overlap(y))) {
        return Some(t.speaker);
    }
    let gap = |t: &Turn| if t.end < a { a - t.end } else { t.start - b };
    turns.iter().filter(|t| gap(t) <= 1.0).min_by(|x, y| gap(x).total_cmp(&gap(y))).map(|t| t.speaker)
}

/// Words with no turn take the previous word's speaker (or the next one's at the start).
fn fill_gaps(labels: &mut [Option<u32>]) {
    for i in 1..labels.len() {
        if labels[i].is_none() {
            labels[i] = labels[i - 1];
        }
    }
    for i in (0..labels.len().saturating_sub(1)).rev() {
        if labels[i].is_none() {
            labels[i] = labels[i + 1];
        }
    }
}

/// Consecutive words with the same speaker, with tiny pieces merged into a neighbour.
fn pieces<'a>(words: &'a [Word], labels: &[Option<u32>]) -> Vec<(Option<u32>, &'a [Word])> {
    let mut runs: Vec<(Option<u32>, usize, usize)> = Vec::new();
    for (i, l) in labels.iter().enumerate() {
        match runs.last_mut() {
            Some(r) if r.0 == *l => r.2 = i + 1,
            _ => runs.push((*l, i, i + 1)),
        }
    }
    // merge short runs into the previous run (or the next, for the first one)
    let mut merged: Vec<(Option<u32>, usize, usize)> = Vec::new();
    for r in runs {
        match merged.last_mut() {
            Some(last) if r.2 - r.1 < MIN_PIECE_WORDS || last.0 == r.0 => last.2 = r.2,
            _ => merged.push(r),
        }
    }
    if merged.len() > 1 && merged[0].2 - merged[0].1 < MIN_PIECE_WORDS {
        let first = merged.remove(0);
        merged[0].1 = first.1;
    }
    merged.into_iter().map(|(l, a, b)| (l, &words[a..b])).collect()
}

/// Speakers present in the transcript, keeping names already given.
pub fn roster(segments: &[Segment], previous: &BTreeMap<String, SpeakerInfo>) -> BTreeMap<String, SpeakerInfo> {
    segments
        .iter()
        .filter_map(|s| s.speaker.clone())
        .filter(|k| k != YOU)
        .map(|k| {
            let info = previous.get(&k).cloned().unwrap_or_default();
            (k, info)
        })
        .collect()
}

/// Display name for a segment's speaker: the given name, else "Person N" / "Pessoa N", else the
/// track (You / Call).
pub fn display_name(seg: &Segment, speakers: &BTreeMap<String, SpeakerInfo>, language: &str) -> String {
    let pt = language == "pt";
    match seg.speaker.as_deref() {
        Some(YOU) => (if pt { "Você" } else { "You" }).into(),
        Some(k) => match speakers.get(k).and_then(|s| s.name.clone()) {
            Some(name) => name,
            None => format!("{} {}", if pt { "Pessoa" } else { "Person" }, k.trim_start_matches('s')),
        },
        None => match seg.track {
            Track::You => (if pt { "Você" } else { "You" }).into(),
            Track::Call => (if pt { "Chamada" } else { "Call" }).into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whisper::approx_words;

    fn seg(start: f64, end: f64, text: &str, track: Track) -> Segment {
        Segment { start, end, text: text.into(), track, words: approx_words(start, end, text), speaker: None }
    }

    #[test]
    fn splits_a_call_segment_where_the_speaker_changes() {
        // 8 words over 0–8 s; speaker 1 until 4 s, speaker 2 after
        let s = seg(0.0, 8.0, "um dois três quatro cinco seis sete oito", Track::Call);
        let turns = [Turn { start: 0.0, end: 4.0, speaker: 1 }, Turn { start: 4.0, end: 8.0, speaker: 2 }];
        let out = assign(&[s], &turns);
        assert_eq!(out.len(), 2, "{out:#?}");
        assert_eq!(out[0].speaker.as_deref(), Some("s1"));
        assert_eq!(out[1].speaker.as_deref(), Some("s2"));
        assert_eq!(format!("{} {}", out[0].text, out[1].text), "um dois três quatro cinco seis sete oito");
        assert!(out[0].end <= out[1].start);
    }

    #[test]
    fn mic_lines_stay_yours_and_blips_dont_split() {
        let you = seg(0.0, 2.0, "oi gente", Track::You);
        // a one-word blip of speaker 2 inside speaker 1's line
        let call = seg(2.0, 7.0, "então vamos falar do relatório", Track::Call);
        let turns = [Turn { start: 2.0, end: 4.0, speaker: 1 }, Turn { start: 4.0, end: 4.9, speaker: 2 }, Turn { start: 4.9, end: 7.0, speaker: 1 }];
        let out = assign(&[you, call], &turns);
        assert_eq!(out.len(), 2, "{out:#?}");
        assert_eq!(out[0].speaker.as_deref(), Some(YOU));
        assert_eq!(out[1].speaker.as_deref(), Some("s1"));
    }

    #[test]
    fn words_outside_any_turn_follow_their_neighbours() {
        let call = seg(10.0, 14.0, "a b c d", Track::Call);
        let turns = [Turn { start: 11.0, end: 13.0, speaker: 3 }];
        let out = assign(&[call], &turns);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].speaker.as_deref(), Some("s3"));
        // nothing near: no speaker
        let far = seg(50.0, 52.0, "x y", Track::Call);
        assert_eq!(assign(&[far], &turns)[0].speaker, None);
    }

    #[test]
    fn roster_keeps_names_and_display_falls_back() {
        let mut segs = assign(&[seg(0.0, 4.0, "a b c d", Track::Call)], &[Turn { start: 0.0, end: 4.0, speaker: 2 }]);
        segs.push(seg(5.0, 6.0, "eu", Track::You));
        let mut prev = BTreeMap::new();
        prev.insert("s2".to_string(), SpeakerInfo { name: Some("Fernanda".into()) });
        prev.insert("s9".to_string(), SpeakerInfo { name: Some("gone".into()) });
        let r = roster(&segs, &prev);
        assert_eq!(r.keys().collect::<Vec<_>>(), vec!["s2"]);
        assert_eq!(display_name(&segs[0], &r, "pt"), "Fernanda");
        assert_eq!(display_name(&segs[0], &BTreeMap::new(), "pt"), "Pessoa 2");
        assert_eq!(display_name(&segs[1], &r, "en"), "You");
    }
}
