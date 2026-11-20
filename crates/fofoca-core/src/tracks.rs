//! Putting the two transcribed tracks back together (spec 018).
//!
//! The mic (`You`) and the call (`Call`) are transcribed separately, so a line's track comes from
//! the file it was transcribed from. This merges the two lists by time and drops mic lines that
//! only repeat the call: without headphones the mic hears the speakers, and Whisper would
//! transcribe that copy as You.

use crate::model::Segment;

/// A mic line is an echo when a call line overlaps it for at least this share of its length…
const ECHO_OVERLAP: f64 = 0.5;
/// …and at least this share of its normalised words appear in that call line.
const ECHO_WORDS: f64 = 0.6;

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
    fn merge_orders_both_tracks_by_start() {
        // a turn-taking chunk: the call speaks first, you answer, the call again
        let call = vec![line(Track::Call, 0.0, 2.0, "E aí Vini,"), line(Track::Call, 3.1, 5.0, "tudo certo por aí?")];
        let mic = vec![line(Track::You, 2.0, 3.0, "tudo bem?"), line(Track::You, 5.2, 6.0, "tudo")];
        let (merged, dropped) = merge(mic, call);
        assert_eq!(dropped, 0);
        let order: Vec<(Track, f64)> = merged.iter().map(|s| (s.track, s.start)).collect();
        assert_eq!(order, vec![(Track::Call, 0.0), (Track::You, 2.0), (Track::Call, 3.1), (Track::You, 5.2)]);
    }
}
