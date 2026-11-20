//! Prompts for the summary model. The grounded, structured version is spec 005.

use std::collections::BTreeMap;

use crate::model::{Segment, SpeakerInfo};
use crate::speakers::display_name;

/// Render the transcript as `[mm:ss] Speaker: text` lines, with speaker names once voices have
/// been separated (spec 003), else You / Call.
pub fn transcript_text(segments: &[Segment], speakers: &BTreeMap<String, SpeakerInfo>, language: &str) -> String {
    segments
        .iter()
        .map(|s| format!("[{}] {}: {}", mmss(s.start), display_name(s, speakers, language), s.text))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn partial(transcript: &str, language: &str) -> String {
    if language == "pt" {
        format!(
            "Transcrição parcial de uma reunião em andamento:\n\n{transcript}\n\n\
             Resuma o que foi dito até agora em no máximo 5 tópicos curtos. \
             Use só fatos ditos; mantenha nomes e números exatos. Responda em português."
        )
    } else {
        format!(
            "Partial transcript of an ongoing meeting:\n\n{transcript}\n\n\
             Summarize what has been said so far in at most 5 short bullet points. \
             Only use facts that were said; keep names and numbers exact."
        )
    }
}

pub fn final_summary(transcript: &str, language: &str) -> String {
    if language == "pt" {
        format!(
            "Transcrição completa de uma reunião:\n\n{transcript}\n\n\
             Escreva o resumo final da reunião em Markdown com: \
             1) **Resumo** em 3 frases; 2) **Pontos principais** em tópicos; \
             3) **Números e datas** citados; 4) **Próximos passos**. \
             Use só fatos ditos; não invente. Responda em português."
        )
    } else {
        format!(
            "Full transcript of a meeting:\n\n{transcript}\n\n\
             Write the final meeting summary in Markdown with: 1) **Summary** in 3 sentences; \
             2) **Key points** as bullets; 3) **Numbers and dates** mentioned; 4) **Next steps**. \
             Only use facts that were said; do not invent."
        )
    }
}

pub fn mmss(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;

    #[test]
    fn renders_speakers_and_times() {
        let segs = vec![
            Segment { start: 0.0, end: 2.0, text: "Oi".into(), track: Track::You, words: vec![], speaker: None },
            Segment { start: 65.4, end: 70.0, text: "Tudo bem".into(), track: Track::Call, words: vec![], speaker: None },
        ];
        assert_eq!(transcript_text(&segs, &BTreeMap::new(), "pt"), "[00:00] Você: Oi\n[01:05] Chamada: Tudo bem");
        let mut named = segs.clone();
        named[1].speaker = Some("s1".into());
        let mut sp = BTreeMap::new();
        sp.insert("s1".to_string(), SpeakerInfo { name: Some("Ana".into()) });
        assert_eq!(transcript_text(&named, &sp, "pt"), "[00:00] Você: Oi\n[01:05] Ana: Tudo bem");
        assert!(partial("x", "en").contains("so far"));
    }
}
