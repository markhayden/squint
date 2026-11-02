# Report viewer

The static viewer under `crates/squint-cli/viewer/` (`index.html`, `viewer.js`, `geometry.js`,
`timeline.js`, `viewer.css`): plain HTML and vanilla JavaScript, no build step, embedded in the
binary so an installed `squint` carries it. It is served live by `report view`, or written as one
self-contained page by `report export`. These are the run's two renders in the description
(table-manners 3.2): a host runs `squint report view {run.dir} --port 0 --frame-ancestor
{frame_ancestor}` (kind `server`, `ready_timeout_s` 10) and `squint report export {run.dir}` (kind
`document`), with the same `squint` executable it serves and each placeholder a whole argument.
Neither modifies the decision directory.

## Serving a run

```
squint report view <decision dir> [--port 0] [--frame-ancestor <origin>] [--open]
```

Binds `127.0.0.1:<port>` (0 picks a free port) and, once listening, prints exactly one stdout
line, the URL (`http://127.0.0.1:<port>/`), and nothing else ever; what it says for people goes to
stderr. It serves until its stdin reaches end of file or it receives SIGTERM or SIGINT, then exits
0. Whoever starts it holds its stdin open for as long as it should serve: in a terminal that is
the terminal (Ctrl-C or Ctrl-D stops it); a background shell job's stdin is `/dev/null`, which
stops it at once, so a script pipes from a sleeping writer (the offline smoke uses a FIFO fed by
`sleep`) and stops it with SIGTERM. With `--open` the system browser is launched on the URL. The
directory must hold `report.json` (`squint report rebuild` makes one from the journals).
`report view` writes nothing anywhere.

`--frame-ancestor <origin>` names the one page origin allowed to frame the viewer, as a host that
embeds it passes its own (`http://127.0.0.1:4801`): `http` or `https`, a host, an optional port,
no path and no trailing slash. Anything else (a path, a wildcard, a CSP keyword, another scheme)
exits 64. Without the flag no page may frame it (`frame-ancestors 'none'`); loopback in general
is never allowed.

The server answers `GET` and `HEAD` only, for exactly these paths:

| Path | Content |
|---|---|
| `/`, `/viewer.js`, `/geometry.js`, `/timeline.js`, `/viewer.css` | the embedded viewer |
| `/report.json` | the decision's report, byte for byte |
| `/run-<n>/journal.jsonl` | a run's journal, for the timeline |
| `/run-<n>/artifacts/<sha256>` | a run artifact: an image the model saw, a request or response body, a tool envelope; typed by its first bytes |

Everything else is `404`, so nothing outside the decision directory (and nothing in it beyond the
report and the artifacts) can be read through it. A request whose `Host` header is not
`127.0.0.1:<port>` or `localhost:<port>` is refused with `421` before any route, so a page from
elsewhere cannot reach the report through a name rebound to loopback. Responses carry
`Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, `Content-Security-Policy:
default-src 'self'; img-src 'self' data:; frame-ancestors <origin>` (`'none'` without
`--frame-ancestor`) and `Connection: close`. The page itself requests nothing but these paths
from the server that served it: no fonts, scripts or images from anywhere else.

The geometry is held to the Rust reference by `viewer_geometry` in the CLI tests: the stored
footprints in `fixtures/render/view-footprints.json` are computed by `frames::sphere` and
recomputed by `geometry.js` under `node` when it is on the path (the comparison is reported as
skipped otherwise).

## Exporting a run: the diagnostics page

```
squint report export <decision dir> [--out <file>] [--open]
```

Writes one HTML file outside the decision directory
(`$XDG_CACHE_HOME/squint/reports/<run id>.html`, or `~/.cache/squint/reports/<run id>.html`, by
default; the run id is the directory's name) and prints only its absolute path on stdout, its size
on stderr. Exit 0 means the file exists whole (it is written beside its destination and renamed
into place). An `--out` inside the decision directory exits 64 before anything is written. The
page opens from disk with no server and needs no file beside it: the viewer's scripts and
stylesheet inlined, and `report.json`, every run's journal and
every run artifact (the images the model saw, each request and response body, each tool
envelope) carried in a `SQUINT_BUNDLE` object the viewer reads instead of the server. A decision
of the arm C run is about 5 MB; the page grows with the images a run saw. It is the page to keep
beside a result or to send to someone.

Both forms carry a **Save as PDF** button that opens the browser's print dialog. Print styles keep
figures and table rows whole and light on paper; the PDF holds what the page shows, so a collapsed
section (rules, job, config as resolved) prints collapsed unless it is opened first.

Both forms open each run with a **timeline**: the funnel of the run as a waterfall from its
journal, one row per step in order (resolve, prepare, reserve, each model turn with its stop
reason and cost, each tool call with its outcome, a repair, the submission, the verdict), each
with its start, duration and status, and a bar on one time axis. A turn spans its send to its
receipt (or the failed attempt), a tool call its dispatch to its return; a call the harness
refused is a point, and a call the tool refused closes with its words. `timeline.js` builds the
rows and is held to a fixed journal by `viewer_timeline` in the CLI tests under `node`.

## What it shows

The viewer declares the `report_version`s it reads (`SUPPORTED_REPORT_VERSIONS` in `viewer.js`,
currently `[1]`) and refuses others with a message. The version names the shape a reader must
accept: fields added since (`frames`, `findings`, an item's `root`) default when absent, so a current binary reads
every version 1 report; an older binary refuses a newer report rather than misread it. For a supported report:

- **Decision**: verdict, exit code, use case, provider, tools, decision runs and agreement, the
  policy, spent against envelope and cap, cache status, prompt and harness versions; a row per
  run; the rules, job and resolved config folded away.
- **Images the model saw**, per run: every frame in `images_sent` with its size, projection and,
  for a crop, its parent and mapping (a box, or the yaw, pitch and fov of a perspective view).
  Every location is drawn twice: on the frame it names, as submitted, and on its root image in
  **raw coordinates**, mapped through the frame tree (`frames` in the report: raw frames and every
  crop a tool returned, with how each sits in its parent). A bbox crop maps by its box; a view
  crop maps by the sphere geometry in `geometry.js`, the same maths as the harness's
  `frames::sphere`: a box or polygon drawn in the view becomes the polygon of its footprint on the
  panorama, cut at the seam and marked clipped when it straddles it, a full-width box when the
  view holds a pole. A `view` location is drawn as the footprint of the view it names. A judged
  location is coloured by its disposition, whether it answers a checklist item or is a finding:
  violations red, observed and allowed changes grey, any other answer blue; tool regions (from
  each call's envelope) are orange. Items that put the same box on the same frame (one defect
  answering several obligations) are drawn once, every item named in the hover text; `whole_image` is a border, `absent` a dashed
  border, a location clipped at its parent is dotted.
- **Tool results**: every call with status, duration, arguments, the items it added or dropped,
  the envelope's summary and region count, and a link to the envelope.
- **Checklist and submission**: the checklist as it stood at the end, joined with each item's
  disposition, confidence, reason and location; the findings.
- **Aggregation**: each item's aggregation row and the finding rows.
- **Turns**: stop reason, tool calls, attempts, cost against bound, usage, history membership,
  provider request id, links to the request and response bodies; repairs; the error if any;
  obligation instances folded away.

Overlays are SVG with a unit viewBox over the image, so a box at `[0.1, 0.1, 0.3, 0.3]` sits at
those fractions of the displayed image whatever its size; the viewer test checks a crop-frame
location lands at the mapped position on the candidate.

## Tests

`cargo nextest run -p squint-cli --features testkit viewer_serve` runs a decision with a returned
crop frame and checks the server's routes, types and refusals, and both renders as a host runs
them: `report view` prints one URL line and nothing more across requests, carries exactly the
`frame-ancestors` it was given, refuses a bad origin with 64, and exits 0 on end of stdin, SIGTERM
and SIGINT; `report export` prints one absolute path outside the decision directory to a page with
no relative `src` or `href`; the decision directory is byte-identical after both. The offline
smoke (`scripts/offline-smoke.sh`, run on every packaged and released binary) starts `report view`
on a minimal directory with its stdin held open by a FIFO fed by `sleep`, checks the URL line, the
page, the report and the framing header, stops it with SIGTERM expecting exit 0, exports the
directory into a scratch cache, and checks that neither changed it; `scripts/test-checks.sh`
proves it fails on fakes that print more than the URL, die on SIGTERM, serve nothing or export no
bundle. The embedded assets are covered by
`scripts/check-packaging.sh`.

## Devtools check (2026-10-27)

On the decision the viewer test produces (`fixtures/scripts/one-call-crop-finding.json` with
`fixtures/tool-scripts/crop-image.json`: a tool returns a crop covering `[0, 0, 0.5, 0.5]` of the
candidate and the model reports a violation at `[0.2, 0.2, 0.6, 0.6]` of that crop), served by
`report view` and opened in Chrome:

- console: no messages;
- network: seven requests, all to `http://127.0.0.1:<port>/` (page, stylesheet, script,
  `report.json`, three artifacts);
- the finding is drawn on the crop panel at fractions `[0.2, 0.2, 0.6, 0.6]` of the image and on
  the candidate panel at `[0.1, 0.1, 0.3, 0.3]`, which is where the report's `root` location puts
  it, measured from the shapes' bounding boxes against the images'.
