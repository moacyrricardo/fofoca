export function mmss(s: number): string {
  const t = Math.max(0, Math.floor(s));
  return `${String(Math.floor(t / 60)).padStart(2, "0")}:${String(t % 60).padStart(2, "0")}`;
}

export function when(iso: string): string {
  const d = new Date(iso);
  const today = new Date();
  const y = new Date(today);
  y.setDate(today.getDate() - 1);
  const hm = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === today.toDateString()) return `Today ${hm}`;
  if (d.toDateString() === y.toDateString()) return `Yesterday ${hm}`;
  return `${d.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" })} ${hm}`;
}

/** Friendly model name from a file stem: "ggml-large-v3-turbo-q5_0" → "large-v3-turbo",
 *  "Qwen3.5-2B-Q4_K_M" → "Qwen3.5-2B". */
export function shortModel(stem: string | undefined): string {
  if (!stem) return "…";
  return stem.replace(/^ggml-/, "").replace(/[-_.](?:q\d[\w]*|Q\d[\w]*|f16|f32|bf16)$/i, "");
}

export function minutes(s: number): string {
  return s < 60 ? `${Math.round(s)} s` : `${Math.round(s / 60)} min`;
}

const esc = (t: string) => t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** Minimal, safe Markdown for model output: headings, bold, bullets, paragraphs. */
export function markdown(src: string): string {
  const out: string[] = [];
  let list = false;
  const inline = (t: string) => esc(t).replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>").replace(/(^|[^*])\*(?!\s)(.+?)\*/g, "$1<em>$2</em>");
  for (const raw of src.split("\n")) {
    const line = raw.trimEnd();
    const bullet = line.match(/^\s*(?:[-*•]|\d+[.)])\s+(.*)$/);
    if (bullet) {
      if (!list) { out.push("<ul>"); list = true; }
      out.push(`<li>${inline(bullet[1])}</li>`);
      continue;
    }
    if (list) { out.push("</ul>"); list = false; }
    const h = line.match(/^#{1,6}\s+(.*)$/);
    if (h) out.push(`<h3>${inline(h[1])}</h3>`);
    else if (/^\*\*[^*]+\*\*:?$/.test(line.trim())) out.push(`<h3>${inline(line.trim().replace(/^\*\*|\*\*:?$/g, ""))}</h3>`);
    else if (line.trim()) out.push(`<p>${inline(line)}</p>`);
  }
  if (list) out.push("</ul>");
  return out.join("");
}

/** Mirrors fofoca-core `labels::normalize`: trimmed, lower-case, whitespace → "-", ≤ 32 chars. */
export function normalizeLabel(s: string): string | null {
  const n = [...s.trim().split(/\s+/).join("-").toLowerCase()].slice(0, 32).join("").replace(/-+$/, "");
  return n || null;
}

export function normalizeLabels(labels: string[]): string[] {
  const out: string[] = [];
  for (const l of labels) {
    const n = normalizeLabel(l);
    if (n && !out.includes(n)) out.push(n);
  }
  return out;
}

/** Pause markers (meeting times) to show before transcript line `i`; `i = segs.length` → after
 *  the last line. A pause sits before the first line that starts at or after it. */
export function pausesBefore(pauses: number[], segs: { start: number }[], i: number): number[] {
  const lo = i === 0 ? -Infinity : segs[i - 1].start;
  const hi = i < segs.length ? segs[i].start : Infinity;
  return pauses.filter((p) => p > lo && p <= hi);
}
