Status: todo
Branch: —

# 024 — GitHub Pages: a landing page with the latest build and install steps

> **Design reference (source of truth for the page):** [`024-assets/github-pages-mock.html`](024-assets/github-pages-mock.html).
> It's a clickable facsimile of `docs/index.html`. The shell bar switches Desktop/Phone and the four
> release-lookup states. Its "ⓘ annotations" mark what's invented.

## Context

The README is the only public face of fo-foca, and it's written for people reading the repo. Someone
who just wants to try the app has to find the Releases page, pick between version-named files, and
read the install steps under the development notes.

The README's roadmap has also drifted from the catalog. It lists calendar (004), grounded notes
(005) and the desktop UI as unfinished, though all three are done. It leaves out labels, import,
replay, tasks, the tray, export and the after-call transcript, which are done too. And it presents
auto setup (002, still `todo`) as a current feature.

Two facts about the releases shape the download button:

- GitHub's `/releases/latest` **skips releases flagged as pre-release**. Today's only release,
  `v0.1.0-pr32`, is *named* a pre-release but isn't flagged, so `/releases/latest` resolves to it.
  Test builds are marked by their tag (a `-suffix` after the version) more reliably than by the flag.
- Asset names carry the version (`fo-foca_0.1.0_amd64.AppImage`), so
  `/releases/latest/download/<name>` breaks at every version bump.

## Decision

1. **Serve `docs/` from `main` with GitHub Pages** ("Deploy from a branch", folder `/docs`). There's
   no build step and no Actions workflow. `docs/.nojekyll` turns Jekyll off, so the `.md` isn't
   rendered as a second page.
2. **One text, one page.**
   - `docs/index.md` is the canonical text: pitch, features, how it works, numbers, install, status.
   - `docs/index.html` is that text laid out per the mock, in the app's design system. It is
     **edited in place** when the text changes. Regenerate it from the `.md` (with `/rich-html`)
     only for a redesign, so diffs stay small and the look stays stable.
3. **The release card reads the GitHub API in the browser.**
   - `GET https://api.github.com/repos/moacyrricardo/fofoca/releases?per_page=10`, then take the
     first release that isn't a draft. This includes flagged pre-releases, which `/releases/latest`
     would skip.
   - Pick the buttons by file ending: `.AppImage`, `.deb`, `fofoca-setup.sh`. Show each file's size.
   - A release is a **test build** when it's flagged as a pre-release **or** its tag has a suffix
     after the version (`v0.1.0-pr32`). It gets a "test build" chip and a note. Otherwise it gets
     a "latest" chip.
   - **Fallback:** if the request fails (offline, the 60/hour per-IP limit, no JS), show a link to
     `/releases`. A `<noscript>` holds the same link.
4. **Sections, top to bottom** (as in the mock): nav · hero (logo, tagline, badges, release card,
   transcript + tasks teaser with invented lines) · features · how it works · reference numbers ·
   install (4 steps, copy buttons, switches for the setup-script variant and AppImage/.deb) ·
   status · footer. The README's Development and Contributing sections stay in the README only.
5. **The roadmap follows the catalog.** A `done` spec is shown as shipped and a `todo` spec as
   planned. A concern is "exploring", and a `doing` spec "in progress". This applies to:
   - README "Status & roadmap",
   - the README's feature list and its "Auto setup" section,
   - the page's features and Status.

   Features that depend on a `todo` spec say "planned". Auto setup (002) is the case today: 015
   ships manual layout choice with a GPU memory meter, while the hardware probe and benchmark
   are 002.
6. **A rule in AGENTS.md keeps them in sync.** A spec's close-out commit updates the README roadmap,
   `docs/index.md` and `docs/index.html` when the spec changes what a user sees or how they
   install. So does any change to install steps or release assets.

## Implementation

- `docs/index.html`: the mock without the shell bar, annotations or toasts.
  - Container queries become `@media` queries.
  - The fake links become real ones: repo, `/releases`, release notes `html_url`, asset
    `browser_download_url`, `specs/CATALOG.md`, issues, LICENSE.
  - The copy buttons use `navigator.clipboard`.
  - Tokens and the `.card .btn .chip .seg .who .dot .spin` rules are copied from `app/src/app.css`.
    A comment at the top says so.
- `docs/assets/`:
  - `logo.jpg`, resized to 1240 px wide.
  - The six feature icons at 128 px.
  - `favicon.png`, made from the transcription icon.

  Pages only serves `docs/`, so these are copies; `assets/` stays the source.
- `docs/index.md`: the same text as plain Markdown, in the same section order.
- `docs/.nojekyll`: empty.
- README:
  - Rewrite "Status & roadmap" from the catalog.
  - Drop "(planned)" from "What it does" and mark the auto-setup row as planned.
  - Add a line pointing to the site.
- AGENTS.md: a **Public docs** section with the rule from Decision 6.
- After merge, which needs the repo owner:
  - `gh api -X POST repos/moacyrricardo/fofoca/pages -f 'source[branch]=main' -f 'source[path]=/docs'`
  - Set the repo homepage to `https://moacyrricardo.github.io/fofoca/`.

## Known Gaps

- **There's no automatic check that the roadmap matches the catalog.** The AGENTS.md rule is the
  only guard.
- **The page has no screenshots.** The teaser is drawn, not captured. A screenshot from the fake
  backend (`npx vite preview`) can replace it later. It must never come from a real meeting.
- **Linux only.** There is no OS detection, because there is only one platform.
- **The page itself is English only**, although the app is pt-BR first.
