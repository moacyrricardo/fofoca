//! Grounded meeting notes (spec 005): the model extracts summary points, decisions and action
//! items as JSON, each citing transcript timestamps; `grounding` keeps only what the cited lines
//! support.

use anyhow::{Context as _, Result};
use std::collections::BTreeMap;

use crate::config::Config;
use crate::grounding::{self, Context, Notes, RawNotes};
use crate::llm::LlmClient;
use crate::model::{MeetingMeta, Segment, SpeakerInfo};
use crate::prompts::{mmss, transcript_text};
use crate::speakers::display_name;

/// Transcripts longer than this share of the model's context are processed in windows.
const CONTEXT_SHARE: f64 = 0.7;
const WINDOW_S: f64 = 20.0 * 60.0;
const MAX_TOKENS: u32 = 1200;

pub fn extract(client: &LlmClient, segments: &[Segment], speakers: &BTreeMap<String, SpeakerInfo>, meta: &MeetingMeta, cfg: &Config) -> Result<Notes> {
    let lang = cfg.language.as_str();
    let mut raw = RawNotes::default();
    let mut speed = Vec::new();
    for window in windows(segments, speakers, cfg) {
        let c = client.complete_json(&prompt(&transcript_text(window, speakers, lang), lang), &grounding::schema(), MAX_TOKENS)?;
        speed.push(c.gen_tokens_per_s);
        let part: RawNotes = serde_json::from_str(&c.text).with_context(|| format!("the model's notes aren't valid JSON: {}", c.text.chars().take(200).collect::<String>()))?;
        merge(&mut raw, part);
    }
    let mut people: Vec<String> = segments.iter().map(|s| display_name(s, speakers, lang)).collect();
    people.extend(meta.attendees.iter().cloned());
    people.sort();
    people.dedup();
    let started_at = chrono::DateTime::parse_from_rfc3339(&meta.started_at).context("meeting start time")?;
    let mut notes = grounding::validate(&raw, &Context { segments, people: &people, started_at });
    notes.model = cfg.llm_name();
    notes.gen_tokens_per_s = if speed.is_empty() { 0.0 } else { speed.iter().sum::<f64>() / speed.len() as f64 };
    Ok(notes)
}

/// The whole transcript, or 20-minute windows when it wouldn't fit comfortably (~3 chars/token).
fn windows<'a>(segments: &'a [Segment], speakers: &BTreeMap<String, SpeakerInfo>, cfg: &Config) -> Vec<&'a [Segment]> {
    let chars = transcript_text(segments, speakers, &cfg.language).len() as f64;
    if chars / 3.0 < cfg.llm.context as f64 * CONTEXT_SHARE || segments.is_empty() {
        return vec![segments];
    }
    let mut out = Vec::new();
    let mut from = 0;
    for i in 1..=segments.len() {
        if i == segments.len() || segments[i].start - segments[from].start >= WINDOW_S {
            out.push(&segments[from..i]);
            from = i;
        }
    }
    out
}

fn merge(into: &mut RawNotes, part: RawNotes) {
    let new = |list: &[grounding::RawItem], text: &str| !list.iter().any(|x| x.text.trim().eq_ignore_ascii_case(text.trim()));
    for it in part.summary {
        if new(&into.summary, &it.text) {
            into.summary.push(it);
        }
    }
    for it in part.decisions {
        if new(&into.decisions, &it.text) {
            into.decisions.push(it);
        }
    }
    for a in part.action_items {
        if !into.action_items.iter().any(|x| x.text.trim().eq_ignore_ascii_case(a.text.trim())) {
            into.action_items.push(a);
        }
    }
}

pub fn prompt(transcript: &str, language: &str) -> String {
    if language == "pt" {
        format!(
            "Transcrição de uma reunião. Cada linha começa com [mm:ss] e o nome de quem falou.\n\n{transcript}\n\n\
             Extraia as notas da reunião em JSON:\n\
             - summary: até 5 frases curtas com o que foi dito de mais importante;\n\
             - decisions: decisões tomadas (lista vazia se nenhuma);\n\
             - action_items: tarefas combinadas; owner = nome de quem vai fazer, como aparece na \
             transcrição, ou null; due = data AAAA-MM-DD só se a data foi dita, senão null.\n\
             Cada item precisa de refs: os horários [mm:ss] das linhas da transcrição que dizem aquilo. \
             Use só o que foi dito, sem deduzir nem calcular; copie números e nomes exatamente. \
             Escreva os textos em português."
        )
    } else {
        format!(
            "Transcript of a meeting. Each line starts with [mm:ss] and who spoke.\n\n{transcript}\n\n\
             Extract the meeting notes as JSON:\n\
             - summary: up to 5 short sentences with the most important things said;\n\
             - decisions: decisions made (empty list if none);\n\
             - action_items: tasks agreed; owner = who will do it, as named in the transcript, or \
             null; due = YYYY-MM-DD only if a date was said, else null.\n\
             Every item needs refs: the [mm:ss] times of the transcript lines that say it. \
             Only use what was said — don't infer or calculate; copy numbers and names exactly."
        )
    }
}

/// Plain Markdown version (stored as the final summary, so older views and exports keep working).
pub fn to_markdown(n: &Notes, language: &str) -> String {
    let pt = language == "pt";
    let refs = |r: &[f64]| r.iter().map(|t| format!("[{}]", mmss(*t))).collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    let mut section = |title: &str, lines: Vec<String>| {
        if !lines.is_empty() {
            out.push_str(&format!("**{title}**\n{}\n\n", lines.join("\n")));
        }
    };
    section(if pt { "Resumo" } else { "Summary" }, n.summary.iter().map(|i| format!("- {} {}", i.text, refs(&i.refs))).collect());
    section(if pt { "Decisões" } else { "Decisions" }, n.decisions.iter().map(|i| format!("- {} {}", i.text, refs(&i.refs))).collect());
    section(
        if pt { "Próximos passos" } else { "Next steps" },
        n.action_items
            .iter()
            .map(|a| {
                let who = [a.owner.clone(), a.due.clone()].into_iter().flatten().collect::<Vec<_>>().join(", ");
                format!("- {}{} {}", a.text, if who.is_empty() { String::new() } else { format!(" ({who})") }, refs(&a.refs))
            })
            .collect(),
    );
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grounding::{Action, Item};
    use crate::model::Track;

    #[test]
    fn long_transcripts_are_windowed() {
        let mut cfg = Config::default();
        let seg = |i: usize| Segment { start: i as f64 * 60.0, end: i as f64 * 60.0 + 50.0, text: "palavra ".repeat(120), track: Track::Call, words: vec![], speaker: None };
        let segs: Vec<Segment> = (0..70).map(seg).collect(); // 70 minutes, ~60k chars
        cfg.llm.context = 32768;
        assert_eq!(windows(&segs, &BTreeMap::new(), &cfg).len(), 1);
        cfg.llm.context = 8192;
        let w = windows(&segs, &BTreeMap::new(), &cfg);
        assert_eq!(w.iter().map(|w| w.len()).sum::<usize>(), 70);
        assert_eq!(w.len(), 4, "20-minute windows: 20 + 20 + 20 + 10");
    }

    #[test]
    fn markdown_cites_times() {
        let n = Notes {
            summary: vec![Item { text: "Q3 abaixo da meta".into(), refs: vec![65.0] }],
            action_items: vec![Action { text: "Enviar follow-up".into(), owner: Some("Você".into()), due: Some("2026-11-19".into()), refs: vec![70.0] }],
            ..Default::default()
        };
        assert_eq!(to_markdown(&n, "pt"), "**Resumo**\n- Q3 abaixo da meta [01:05]\n\n**Próximos passos**\n- Enviar follow-up (Você, 2026-11-19) [01:10]");
    }
}
