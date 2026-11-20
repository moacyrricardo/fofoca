//! Putting the two transcribed tracks back together (spec 018).
//!
//! The mic (`You`) and the call (`Call`) are transcribed separately, so a line's track comes from
//! the file it was transcribed from. This merges the two lists by time and drops mic lines that
//! only repeat the call: without headphones the mic hears the speakers, and Whisper would
//! transcribe that copy as You.

use crate::model::{Segment, Track, Word};
use crate::whisper::RawSegment;

/// A mic line is an echo when a call line overlaps it for at least this share of its length…
const ECHO_OVERLAP: f64 = 0.5;
/// …and at least this share of its normalised words appear in that call line.
const ECHO_WORDS: f64 = 0.6;

/// The last `n` characters of `s` (Whisper's prompt is the tail of the transcript).
pub fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(n)).collect()
}

/// Lines from one request, without mic lines that only repeat a call line already in `known`.
/// Returns the kept lines and how many were dropped as echoes.
pub fn drop_echoes(new: Vec<Segment>, known: &[Segment]) -> (Vec<Segment>, usize) {
    let before = new.len();
    let kept: Vec<Segment> = new.into_iter().filter(|m| m.track != Track::You || !known.iter().any(|c| c.track == Track::Call && is_echo(m, c))).collect();
    let dropped = before - kept.len();
    (kept, dropped)
}

/// Put `line` among `lines` (sorted by start) after every line starting at or before it, and
/// return where it went (spec 019: lines arrive out of order, a long call turn can finish after
/// your later reply). Equal starts keep their arrival order.
pub fn insert_by_start(lines: &mut Vec<Segment>, line: Segment) -> usize {
    let at = lines.partition_point(|l| l.start <= line.start);
    lines.insert(at, line);
    at
}

/// A Whisper line from `track`, moved by `offset` seconds.
pub fn segment(r: RawSegment, track: Track, offset: f64) -> Segment {
    Segment {
        start: offset + r.start,
        end: offset + r.end,
        words: r.words.into_iter().map(|w| Word { start: offset + w.start, end: offset + w.end, ..w }).collect(),
        text: r.text,
        track,
        speaker: None,
    }
}

/// Both tracks' lines in time order, without mic echoes of the call. Returns the merged lines and
/// how many mic lines were dropped as echoes. Lines starting together keep mic before call.
pub fn merge(mic: Vec<Segment>, call: Vec<Segment>) -> (Vec<Segment>, usize) {
    let before = mic.len();
    let mic: Vec<Segment> = mic.into_iter().filter(|m| !call.iter().any(|c| is_echo(m, c))).collect();
    let dropped = before - mic.len();
    let mut out = mic;
    out.extend(call);
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    (out, dropped)
}

/// Whether mic line `mic` is the call line `call` heard through the speakers.
pub fn is_echo(mic: &Segment, call: &Segment) -> bool {
    let len = mic.end - mic.start;
    let overlap = mic.end.min(call.end) - mic.start.max(call.start);
    let overlaps = if len > 0.0 { overlap >= ECHO_OVERLAP * len } else { overlap >= 0.0 };
    if !overlaps {
        return false;
    }
    let mine = words(&mic.text);
    if mine.is_empty() {
        return false;
    }
    let theirs = words(&call.text);
    let found = mine.iter().filter(|w| theirs.contains(w)).count();
    found as f64 >= ECHO_WORDS * mine.len() as f64
}

/// Lower-case words without accents or punctuation: `"Não, é."` → `["nao", "e"]`.
fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).map(unaccent).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect()
}

fn unaccent(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        c => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;

    fn line(track: Track, start: f64, end: f64, text: &str) -> Segment {
        Segment { start, end, text: text.into(), track, words: vec![], speaker: None }
    }

    #[test]
    fn normalises_case_accents_and_punctuation() {
        assert_eq!(words("Não, É  ação!"), vec!["nao", "e", "acao"]);
        assert!(words(" ... ").is_empty());
    }

    #[test]
    fn exact_echo_is_dropped() {
        let call = line(Track::Call, 10.0, 13.0, "A gente fecha o relatório na sexta.");
        let echo = line(Track::You, 10.2, 13.1, "a gente fecha o relatório na sexta");
        assert!(is_echo(&echo, &call));
        let (merged, dropped) = merge(vec![echo], vec![call.clone()]);
        assert_eq!((merged, dropped), (vec![call], 1));
    }

    #[test]
    fn partial_echo_is_dropped() {
        // Whisper heard most of the call line on the mic, with one word wrong and one missing.
        let call = line(Track::Call, 20.0, 24.0, "Então a gente manda o contrato amanhã cedo");
        let echo = line(Track::You, 20.5, 23.5, "então gente manda o contato amanhã cedo");
        assert!(is_echo(&echo, &call), "6 of 7 words match");
    }

    #[test]
    fn overlapping_reply_is_kept() {
        // talking over each other: different words at the same time are two people
        let call = line(Track::Call, 30.0, 33.0, "Você consegue mandar até sexta?");
        let reply = line(Track::You, 31.0, 33.5, "Sim, mando hoje mesmo.");
        assert!(!is_echo(&reply, &call));
        let (merged, dropped) = merge(vec![reply.clone()], vec![call.clone()]);
        assert_eq!((merged, dropped), (vec![call, reply], 0));
    }

    #[test]
    fn same_word_at_another_time_is_kept() {
        let call = line(Track::Call, 40.0, 40.6, "Sim.");
        let you = line(Track::You, 42.0, 42.5, "sim");
        assert!(!is_echo(&you, &call));
        // a short overlap isn't enough either
        let late = line(Track::You, 40.4, 41.4, "sim");
        assert!(!is_echo(&late, &call), "20 % overlap");
    }

    #[test]
    fn lines_are_inserted_by_start() {
        let mut lines = vec![line(Track::Call, 0.0, 2.0, "Oi"), line(Track::You, 12.0, 13.0, "Combinado.")];
        // their long turn finished after your later reply
        assert_eq!(insert_by_start(&mut lines, line(Track::Call, 3.0, 11.5, "Então, sobre o contrato…")), 1);
        assert_eq!(insert_by_start(&mut lines, line(Track::You, 12.0, 12.5, "Sim")), 3, "equal starts keep arrival order");
        assert_eq!(insert_by_start(&mut lines, line(Track::You, 20.0, 21.0, "Tchau")), 4);
        let order: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(order, vec!["Oi", "Então, sobre o contrato…", "Combinado.", "Sim", "Tchau"]);
    }

    #[test]
    fn tail_is_char_safe() {
        assert_eq!(tail_chars("ação", 2), "ão");
        assert_eq!(tail_chars("ab", 10), "ab");
    }

    #[test]
    fn echoes_of_known_call_lines_are_dropped() {
        let known = vec![line(Track::Call, 10.0, 13.0, "A gente fecha o relatório na sexta."), line(Track::You, 13.5, 14.0, "Fechado.")];
        let new = vec![line(Track::You, 10.2, 13.1, "a gente fecha o relatório na sexta"), line(Track::You, 14.5, 16.0, "Mando amanhã.")];
        let (kept, dropped) = drop_echoes(new, &known);
        assert_eq!(dropped, 1);
        assert_eq!(kept.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), vec!["Mando amanhã."]);
        // call lines are never dropped
        let call = vec![line(Track::Call, 10.2, 13.1, "a gente fecha o relatório na sexta")];
        assert_eq!(drop_echoes(call.clone(), &known), (call, 0));
    }

    #[test]
    fn merge_orders_both_tracks_by_start() {
        // a turn-taking chunk: the call speaks first, you answer, the call again
        let call = vec![line(Track::Call, 0.0, 2.0, "Oi, como vai?"), line(Track::Call, 3.1, 5.0, "tudo certo por aí?")];
        let mic = vec![line(Track::You, 2.0, 3.0, "tudo certo?"), line(Track::You, 5.2, 6.0, "tudo")];
        let (merged, dropped) = merge(mic, call);
        assert_eq!(dropped, 0);
        let order: Vec<(Track, f64)> = merged.iter().map(|s| (s.track, s.start)).collect();
        assert_eq!(order, vec![(Track::Call, 0.0), (Track::You, 2.0), (Track::Call, 3.1), (Track::You, 5.2)]);
    }
}
