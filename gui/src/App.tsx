import { type ReactNode, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  type Audit,
  type Batch,
  type BatchDetail,
  type Day,
  type Grant,
  type Metric,
  type MetricInfo,
  OpError,
  type Point,
  type Source,
  type Status,
  agentClients,
  connectAgent,
  onChange,
  chooseImportFile,
  chooseSavePath,
  corePath,
  initProfile,
  isDesktop,
  menuOwnsKeys,
  onFileDrop,
  onMenu,
  setBadge,
  onStatus,
  op,
  unlock,
} from "./api";
import { DailyChart, type Pt, Spark } from "./charts";
import { CLIENTS, type Client } from "./agents";
import { KEY, nameOf } from "./names";
import {
  GROUPS,
  type ImportKind,
  KIND_LABEL,
  ago,
  asDuration,
  day,
  dateIn,
  dayMs,
  isLabGroup,
  PER_MS_UNITS,
  shortDate,
  dayUnit,
  dayValue,
  explain,
  fileName,
  groupLabel,
  importKind,
  num,
  shown,
  sourceKind,
  unitLabel,
  when,
} from "./format";

type Page = "readings" | "add" | "sources" | "review" | "agents" | "backup";
const PAGES: { key: Page; label: string }[] = [
  { key: "readings", label: "Readings" },
  { key: "add", label: "Add data" },
  { key: "sources", label: "Sources" },
  { key: "backup", label: "Backup" },
  { key: "agents", label: "Agent access" },
];
const MAC = typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent);
const MOD = MAC ? "⌘" : "Ctrl+";

/** Sidebar icons: 16px outline, drawn in currentColor. */
const ICON: Record<string, ReactNode> = {
  readings: <path d="M2 12h3l2-6 3 9 2-5h2" />,
  add: <path d="M8 3v10M3 8h10" />,
  sources: (
    <>
      <ellipse cx="8" cy="4" rx="5" ry="2" />
      <path d="M3 4v8c0 1.1 2.2 2 5 2s5-.9 5-2V4M3 8c0 1.1 2.2 2 5 2s5-.9 5-2" />
    </>
  ),
  agents: (
    <>
      <rect x="3" y="5" width="10" height="8" rx="2" />
      <path d="M8 2v3M6 9h0M10 9h0" />
    </>
  ),
  backup: <path d="M3 10v3h10v-3M8 2v8M5 7l3 3 3-3" />,
  review: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M8 5v3.5l2 1.5" />
    </>
  ),
};
const Icon = ({ k }: { k: string }) => (
  <svg className="icon" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
    {ICON[k]}
  </svg>
);

/** The last section opened, remembered on this computer (a convenience: may be unavailable). */
function savedPage(): Page {
  try {
    const p = localStorage.getItem("page") as Page | null;
    return p && PAGES.some((x) => x.key === p) ? p : "readings";
  } catch {
    return "readings";
  }
}

/** Loads `fn` now and after every committed write; returns [data, error, reload]. */
function useOp<T>(fn: () => Promise<T>, deps: unknown[]): [T | undefined, string | undefined, () => void] {
  const [data, setData] = useState<T>();
  const [err, setErr] = useState<string>();
  const [tick, setTick] = useState(0);
  const reload = useCallback(() => setTick((t) => t + 1), []);
  // biome-ignore lint: deps are the caller's
  useEffect(() => {
    let live = true;
    fn().then(
      (d) => live && (setData(d), setErr(undefined)),
      (e: unknown) => live && setErr(e instanceof OpError ? `${e.code}: ${e.message}` : String(e)),
    );
    return () => {
      live = false;
    };
  }, [...deps, tick]);
  useEffect(() => onChange(reload), [reload]);
  return [data, err, reload];
}

function FirstRun({ done }: { done: () => void }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();
  return (
    <main className="gate">
      <h2>Welcome</h2>
      <p className="lede">
        Your health data will live in one encrypted file on this computer. The key is kept in your system keychain. Nothing is sent anywhere.
      </p>
      <Problem text={err} />
      <button
        type="button"
        className="primary"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          try {
            await initProfile();
            done();
          } catch (e) {
            setErr(e instanceof OpError ? e.message : String(e));
            setBusy(false);
          }
        }}
      >
        {busy ? "Creating…" : "Create my health store"}
      </button>
    </main>
  );
}

function Unlock({ done }: { done: () => void }) {
  const [p, setP] = useState("");
  const [err, setErr] = useState<string>();
  return (
    <main className="gate">
      <form
        className="form"
        onSubmit={async (e) => {
          e.preventDefault();
          try {
            await unlock(p);
            setP("");
            done();
          } catch (x) {
            setErr(x instanceof OpError && x.code === "bad_passphrase" ? "That passphrase is not right." : String(x));
          }
        }}
      >
        <h2>Health is locked</h2>
        <p className="lede">Your data is encrypted with your passphrase. It locks again after a while without use.</p>
        <label>
          Passphrase
          <input type="password" autoComplete="current-password" autoFocus value={p} onChange={(e) => setP(e.target.value)} required />
        </label>
        <Problem text={err} />
        <button type="submit" className="primary">
          Unlock
        </button>
      </form>
    </main>
  );
}

export function App() {
  const [page, setPage] = useState<Page>(savedPage);
  // ⌘F asks Readings to focus its search; Readings clears the request once its search box exists
  const [find, setFind] = useState(false);
  useEffect(() => {
    try {
      if (PAGES.some((x) => x.key === page)) localStorage.setItem("page", page);
    } catch {}
  }, [page]);
  const [metric, setMetric] = useState<string>();
  const [status, statusErr, reload] = useOp(() => op<Status>("status"), []);
  const [latest, latestErr] = useOp(() => op<Point[]>("obs.latest"), []);
  const [cats, setCats] = useState(true);
  const [core, setCore] = useState<{ up: boolean; error?: string; need_init?: boolean }>({ up: true });
  const [dropped, setDropped] = useState<string>();
  const [hovering, setHovering] = useState(false);
  // a file dropped anywhere on the window opens Import with it
  useEffect(
    () =>
      onFileDrop({
        over: setHovering,
        drop: (paths) => {
          if (paths[0]) {
            setDropped(paths[0]);
            setMetric(undefined);
            setPage("add");
          }
        },
      }),
    [],
  );
  useEffect(() => onStatus((s) => (setCore(s), reload())), [reload]);
  useEffect(() => setBadge(status?.reviews ?? 0), [status?.reviews]);
  // the same commands from the macOS menu or, elsewhere, from the keyboard
  const command = useCallback((id: string) => {
    if (id !== "back") setMetric(undefined); // a section change or import replaces whatever sheet is open
    if (PAGES.some((x) => x.key === id)) setPage(id as Page);
    else if (id === "back") setMetric(undefined);
    else if (id === "find") {
      setPage("readings");
      setFind(true);
    } else if (id === "import")
      chooseImportFile().then((f) => {
        if (f) {
          setDropped(f);
          setPage("add");
        }
      });
  }, []);
  useEffect(() => onMenu(command), [command]);
  useEffect(() => {
    if (menuOwnsKeys) return;
    const keys: Record<string, string> = { "1": "readings", "2": "add", "3": "sources", "4": "backup", "5": "agents", f: "find", o: "import", "[": "back" };
    const down = (e: KeyboardEvent) => {
      const id = (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey ? keys[e.key.toLowerCase()] : e.altKey && e.key === "ArrowLeft" ? "back" : undefined;
      if (id && !(id === "import" && !isDesktop)) {
        e.preventDefault();
        command(id);
      }
    };
    window.addEventListener("keydown", down);
    return () => window.removeEventListener("keydown", down);
  }, [command]);
  // desktop: no browser context menu (Reload, Inspect) except where text is edited or selected
  useEffect(() => {
    if (!isDesktop) return;
    const menu = (e: MouseEvent) => {
      const t = e.target as HTMLElement;
      if (!t.closest("input, textarea, pre, code") && !window.getSelection()?.toString()) e.preventDefault();
    };
    document.addEventListener("contextmenu", menu);
    return () => document.removeEventListener("contextmenu", menu);
  }, []);
  const starting = !core.up || statusErr?.startsWith("unavailable:");
  // the start-up event can fire before the listener exists: ask again until the core answers
  useEffect(() => {
    if (!starting) return;
    const t = setInterval(reload, 1000);
    return () => clearInterval(t);
  }, [starting, reload]);
  if (statusErr?.startsWith("locked:")) return <Unlock done={reload} />;
  if (core.need_init || statusErr?.includes("no_profile")) return <FirstRun done={reload} />;
  if (starting)
    return (
      <main className="gate" role="status">
        <h2>Starting the health core…</h2>
        <p className="lede">{core.error ?? "This takes a moment. Your data is safe on disk."}</p>
      </main>
    );
  const open = (code: string) => setMetric(code);
  const present = GROUPS.filter((g) => latest?.some((p) => p.group === g.key));
  const navItem = (p: { key: Page; label: string }, hint?: string) => (
    <li key={p.key}>
      <button
        type="button"
        className={page === p.key ? "on" : ""}
        aria-current={page === p.key ? "page" : undefined}
        onClick={() => setPage(p.key)}
        title={hint}
      >
        <Icon k={p.key} />
        <span className="label">{p.label}</span>
        {p.key === "review" && status && status.reviews > 0 && <span className="count">{status.reviews}</span>}
      </button>
    </li>
  );
  const hint = (k: Page) => {
    const i = PAGES.findIndex((x) => x.key === k);
    return `${PAGES[i]!.label} (${MOD}${i + 1})`;
  };
  return (
    <div className={`app${isDesktop ? " desktop" : ""}${isDesktop && MAC ? " mac" : ""}`}>
      <div className="backdrop" aria-hidden="true" />
      {isDesktop && MAC && (
        <>
          <div className="titlebar" data-tauri-drag-region />
          <button type="button" className="toolbar-btn" aria-label="Add data" title={`Add data (${MOD}2)`} onClick={() => command("add")}>
            <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
              <path d="M8 3v10M3 8h10" />
            </svg>
          </button>
        </>
      )}
      {hovering && (
        <div className="drop-overlay" aria-hidden="true">
          <p>Drop to import</p>
          <p className="fine">Apple Health export, Health backup, WHOOP data</p>
        </div>
      )}
      <nav className="rail" aria-label="Sections">
        <ul>
          {navItem(PAGES[0]!, hint("readings"))}
          {status && status.reviews > 0 && navItem({ key: "review", label: "Waiting for you" })}
        </ul>
        {present.length > 0 && (
          <div className="rail-section">
            <button type="button" className="rail-head" aria-expanded={cats} onClick={() => setCats(!cats)}>
              Categories
              <Chevron open={cats} />
            </button>
            {cats && (
              <ul>
                {present.map((g) => (
                  <li key={g.key}>
                    <button
                      type="button"
                      onClick={() => {
                        setPage("readings");
                        requestAnimationFrame(() => document.getElementById(`g-${g.key}`)?.scrollIntoView({ behavior: "smooth", block: "start" }));
                      }}
                    >
                      <GroupIcon k={g.key} />
                      <span className="label">{g.label}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
        <div className="rail-section">
          <p className="rail-head">Your data</p>
          <ul>
            {navItem(PAGES[1]!, hint("add"))}
            {navItem(PAGES[2]!, hint("sources"))}
            {navItem(PAGES[3]!, hint("backup"))}
          </ul>
        </div>
        <div className="rail-section">
          <p className="rail-head">Sharing</p>
          <ul>{navItem(PAGES[4]!, hint("agents"))}</ul>
        </div>
        {status && (
          <p className="fine rail-foot">
            {status.observations.toLocaleString()} readings from {status.sources} {status.sources === 1 ? "source" : "sources"}, stored only on this
            computer
          </p>
        )}
      </nav>
      <main className="main">
        <div className="view" key={page}>
          {page === "readings" && (
            <Readings latest={latest} err={latestErr} open={open} go={setPage} reviews={status?.reviews ?? 0} find={find} found={() => setFind(false)} />
          )}
          {page === "add" && <AddData file={dropped} onFile={setDropped} go={setPage} done={open} />}
          {page === "sources" && <Sources go={setPage} />}
          {page === "review" && <Review />}
          {page === "agents" && (
            <>
              <Agents />
              <AuditLog />
            </>
          )}
          {page === "backup" && <Backup go={setPage} />}
        </div>
      </main>
      {metric && <Sheet label="History" close={() => setMetric(undefined)}>
        <Record code={metric} pick={setMetric} />
      </Sheet>}
    </div>
  );
}

const Chevron = ({ open }: { open: boolean }) => (
  <svg className={`chevron${open ? " open" : ""}`} viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
    <path d="M6 4l4 4-4 4" />
  </svg>
);

/** A layer over the window (native <dialog>: focus stays inside, Esc closes). */
function Sheet({ label, close, children }: { label: string; close: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  const pressedOutside = useRef(false);
  // a layout effect: its cleanup runs before React removes the <dialog>, so close() can hand focus back to
  // whatever had it when the sheet opened (removing an open modal leaves focus on <body>)
  useLayoutEffect(() => {
    const back = document.activeElement as HTMLElement | null;
    const d = ref.current;
    if (d && !d.open) d.showModal();
    return () => {
      d?.close();
      back?.focus?.();
    };
  }, []);
  return (
    <dialog
      ref={ref}
      className="sheet"
      aria-label={label}
      onCancel={(e) => {
        e.preventDefault();
        close();
      }}
      // close on a click on the backdrop itself, not on a text selection dragged out of the sheet
      onMouseDown={(e) => {
        pressedOutside.current = e.target === ref.current;
      }}
      onClick={(e) => e.target === ref.current && pressedOutside.current && close()}
    >
      <div className="sheet-body">
        <button type="button" className="close" aria-label="Close" onClick={close}>
          <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
        {children}
      </div>
    </dialog>
  );
}

/** "code: message" from the core, in words a person can act on. */
function human(text: string): string {
  const m = /^([a-z_]+): ?(.*)$/s.exec(text);
  return m ? explain(m[1]!, m[2]) : text;
}
const errText = (e: unknown) => (e instanceof OpError ? explain(e.code, e.message) : String(e));

function Problem({ text }: { text?: string }) {
  return text ? (
    <p className="problem" role="alert">
      {human(text)}
    </p>
  ) : null;
}

function Copy({ text, label }: { text: string; label: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(text);
          setDone(true);
          setTimeout(() => setDone(false), 2500);
        } catch {
          setDone(false);
        }
      }}
    >
      {done ? "Copied" : label}
    </button>
  );
}

function PasswordField({ label, value, onChange, auto }: { label: string; value: string; onChange: (v: string) => void; auto: string }) {
  const [show, setShow] = useState(false);
  const id = useId();
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <span className="with-toggle">
        <input id={id} type={show ? "text" : "password"} autoComplete={auto} value={value} onChange={(e) => onChange(e.target.value)} required />
        <button type="button" className="link" aria-label={show ? `Hide ${label.toLowerCase()}` : `Show ${label.toLowerCase()}`} aria-pressed={show} onClick={() => setShow(!show)}>
          {show ? "Hide" : "Show"}
        </button>
      </span>
    </div>
  );
}

/** Where a file comes from: native dialog and drag and drop on desktop; a typed path in the browser preview. */
function FilePick({ path, onPath }: { path?: string; onPath: (p: string) => void }) {
  if (!isDesktop)
    return (
      <label className="block">
        File path
        <input value={path ?? ""} onChange={(e) => onPath(e.target.value)} placeholder="/Users/you/Downloads/export.zip" />
      </label>
    );
  return (
    <div className="dropzone">
      {path ? (
        <p>
          <strong>{fileName(path)}</strong>
          <br />
          <span className="fine">{path}</span>
        </p>
      ) : (
        <p>Drag a file here, or anywhere in this window.</p>
      )}
      <button
        type="button"
        className={path ? "" : "primary"}
        onClick={async () => {
          const p = await chooseImportFile();
          if (p) onPath(p);
        }}
      >
        {path ? "Choose another file…" : "Choose file…"}
      </button>
    </div>
  );
}

interface FileResult {
  written: number;
  added: number;
  sources: number;
  apple?: { records: number; broken: number; skipped_types: Record<string, number> };
}
interface BackupResult {
  added_imports: number;
  erased_imports: number;
  refused_imports: number;
  refused_corrections: number;
  skipped_metrics: string[];
  missing_blobs: string[];
  restored_blobs: number;
}

function AddData({ file, onFile, go, done }: { file?: string; onFile: (p?: string) => void; go: (p: Page) => void; done: (code: string) => void }) {
  return (
    <section className="page narrow">
      <h2>Add data</h2>
      <p className="lede">Bring in a file, or write down a reading yourself.</p>
      <div className="stack">
        <Import file={file} onFile={onFile} go={go} />
        <Enter done={done} />
      </div>
    </section>
  );
}

function Import({ file, onFile, go }: { file?: string; onFile: (p?: string) => void; go: (p: Page) => void }) {
  const [password, setPassword] = useState("");
  const [source, setSource] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();
  const [done, setDone] = useState<ReactNode>();
  const kind: ImportKind | null = file ? importKind(file) : null;
  useEffect(() => {
    setErr(undefined);
    setDone(undefined);
    setPassword("");
    setSource(file ? fileName(file).replace(/\.jsonl$/i, "") : "");
  }, [file]);
  const run = async () => {
    if (!file || !kind) return;
    setBusy(true);
    setErr(undefined);
    setDone(undefined);
    try {
      if (kind === "backup") {
        const r = await op<BackupResult>("backup.import", { path: file, password });
        const notes: string[] = [];
        if (r.refused_imports + r.refused_corrections) notes.push(`${r.refused_imports + r.refused_corrections} entries were refused because their dates lie in the future, which means a damaged or altered file.`);
        if (r.skipped_metrics.length) notes.push(`${r.skipped_metrics.length} measures were skipped because their unit differs on this computer: ${r.skipped_metrics.join(", ")}.`);
        if (r.missing_blobs.length) notes.push(`${r.missing_blobs.length} original files (such as lab report photos) are not in this backup; the readings from them are.`);
        setDone(
          <>
            <p>
              Restored: {r.added_imports} imports added
              {r.erased_imports ? `, ${r.erased_imports} removed because they were deleted on the other device` : ""}. Nothing here was overwritten.
            </p>
            {notes.map((n) => (
              <p key={n} className="fine">
                {n}
              </p>
            ))}
          </>,
        );
      } else {
        const r = await op<FileResult>(`import.${kind}`, kind === "rows" ? { path: file, source } : { path: file });
        const broken = r.apple?.broken ?? 0;
        setDone(
          <>
            <p>
              Imported {r.written.toLocaleString()} readings from {r.sources} {r.sources === 1 ? "device" : "devices"}
              {r.added < r.written ? ` (${(r.written - r.added).toLocaleString()} were already here and were updated)` : ""}.
            </p>
            {broken > 0 && <p className="fine">{broken.toLocaleString()} records had a bad date or value and were left out.</p>}
          </>,
        );
      }
      setPassword("");
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="page narrow">
      <h2>Import a file</h2>
      <p className="lede">An Apple Health export, a Health backup (.zuhealth) or WHOOP data. Importing the same file again updates it; nothing is doubled.</p>
      <FilePick path={file} onPath={onFile} />
      {file && !kind && <Problem text="Health can't read this kind of file. Choose a .zip from Apple Health, a .zuhealth backup, or a WHOOP .json file." />}
      {file && kind && (
        <form
          className="form"
          aria-busy={busy}
          onSubmit={(e) => {
            e.preventDefault();
            run();
          }}
        >
          <p className="fine">{KIND_LABEL[kind]}</p>
          {kind === "backup" && <PasswordField label="Backup password" value={password} onChange={setPassword} auto="current-password" />}
          {kind === "rows" && (
            <label>
              Where these readings come from
              <input value={source} onChange={(e) => setSource(e.target.value)} required />
            </label>
          )}
          <Problem text={err} />
          <button type="submit" className="primary" disabled={busy}>
            {busy ? (kind === "backup" ? "Restoring…" : "Importing…") : kind === "backup" ? "Restore backup" : "Import"}
          </button>
          {busy && <p className="fine">Large exports can take a few minutes. Health answers again when the import is done; keep this page open to see the result.</p>}
        </form>
      )}
      {done && (
        <div className="group notice" role="status">
          {done}
          <div className="actions">
            <button type="button" className="primary" onClick={() => go("readings")}>
              See my readings
            </button>
            <button type="button" onClick={() => onFile(undefined)}>
              Import another file
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

const DAY = 86_400_000;
const toPts = (agg: string, days: Day[]): Pt[] =>
  days.flatMap((d) => {
    const v = dayValue(agg, d);
    return v === null ? [] : [{ day: d.day, v, lo: agg === "instant" ? d.min : null, hi: agg === "instant" ? d.max : null }];
  });

/** The last 30 days of a measure, reaching back to its latest reading's day when that is older
 *  (one request per tile, refreshed after writes). */
function useMonth(p: Point): Day[] | undefined {
  const from = day(Math.min(Date.now() - 30 * DAY, p.start_ms));
  return useOp(() => op<Day[]>("daily.query", { metric: p.metric, from }), [p.metric, from])[0];
}

function Trend({ p, days }: { p: Point; days?: Day[] }) {
  const pts = days ? toPts(p.agg, days) : [];
  return pts.length >= 2 ? <Spark pts={pts} label={nameOf(p.metric, p.name)} /> : null;
}

/** What to show for a reading. Summed measures (steps, energy) show their latest day's total, not the last slice. */
function display(p: Point, days?: Day[]): { value: string; unit: string; note?: string } {
  if (p.agg === "cumulative" && days?.length) {
    const last = days[days.length - 1]!;
    const today = last.day === day(Date.now());
    const note = today ? "today" : ago(dayMs(last.day));
    const asTime = asDuration(last.sum ?? 0, p.unit, true);
    return asTime ? { value: asTime, unit: "", note } : { value: num(last.sum ?? 0), unit: unitLabel(p.unit), note };
  }
  return shown(p);
}

/** Generic device names ("OCR", "photo") read better as their kind. */
function sourceLabel(src?: Source): string {
  if (!src) return "Source";
  if (src.connector === "manual") return "Entered by you";
  const generic = ["ocr", "photo", src.connector].includes(src.name.toLowerCase());
  return generic ? sourceKind(src.connector) : src.name;
}

const STALE_DAYS = 60;

/** Out-of-range mark from a lab sheet (H/L printed next to the value). */
function Flag({ p }: { p: Point }) {
  const f = (p.detail?.flag ?? (p.detail?.printed as { flag?: string } | undefined)?.flag) as string | undefined;
  if (p.corrected || (f !== "H" && f !== "L")) return null;
  return (
    <span className="flag" title={f === "H" ? "Above the reference range" : "Below the reference range"}>
      {f === "H" ? "↑ high" : "↓ low"}
    </span>
  );
}

const isStale = (p: Point) => Date.now() - p.start_ms > STALE_DAYS * DAY;

/** Category symbols, tinted per category (the tint marks the category only; text stays in ink). */
const GROUP_ICON: Record<string, ReactNode> = {
  body: (
    <>
      <circle cx="8" cy="3.5" r="1.8" />
      <path d="M4 7h8M8 7v4M8 11l-2.5 3.5M8 11l2.5 3.5" />
    </>
  ),
  cardio: <path d="M8 13.5S2.5 10 2.5 6.2A2.7 2.7 0 0 1 8 5a2.7 2.7 0 0 1 5.5 1.2C13.5 10 8 13.5 8 13.5z" />,
  sleep: <path d="M12.5 10A5.5 5.5 0 0 1 6 3.5a5.5 5.5 0 1 0 6.5 6.5z" />,
  activity: <path d="M9 2L4 9h4l-1 5 5-7H8z" />,
  lab: <path d="M6 2h4M6.5 2v4L3 13a1 1 0 0 0 .9 1.4h8.2A1 1 0 0 0 13 13L9.5 6V2" />,
  infectious: <path d="M8 2l5 2v4c0 3-2.2 5.2-5 6-2.8-.8-5-3-5-6V4z" />,
  tumor: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <circle cx="8" cy="8" r="2" />
    </>
  ),
  repro: (
    <>
      <circle cx="8" cy="6" r="3.5" />
      <path d="M8 9.5V15M5.5 12.5h5" />
    </>
  ),
  derived: <path d="M8 2v3M8 11v3M2 8h3M11 8h3M4 4l2 2M10 10l2 2M12 4l-2 2M6 10l-2 2" />,
};
const GroupIcon = ({ k }: { k: string }) => (
  <svg className={`icon tint-${k}`} viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
    {GROUP_ICON[k] ?? <rect x="3" y="3" width="10" height="10" rx="2.5" />}
  </svg>
);

/** One measure as a tile: its name, the value large, and the month's trend or when it was taken. */
function Tile({ p, open, lab }: { p: Point; open: (code: string) => void; lab: boolean }) {
  return p.agg === "cumulative" ? <SummedTile p={p} open={open} /> : <TileBody p={p} open={open} s={shown(p)} days={undefined} lab={lab} />;
}

function SummedTile({ p, open }: { p: Point; open: (code: string) => void }) {
  const days = useMonth(p);
  return <TileBody p={p} open={open} s={display(p, days)} days={days} lab={false} />;
}

function TrendTile({ p, open, s }: { p: Point; open: (code: string) => void; s: { value: string; unit: string; note?: string } }) {
  const days = useMonth(p);
  return <TileBody p={p} open={open} s={s} days={days} lab={false} />;
}

function TileBody({
  p,
  open,
  s,
  days,
  lab,
}: { p: Point; open: (code: string) => void; s: { value: string; unit: string; note?: string }; days?: Day[]; lab: boolean }) {
  const old = isStale(p);
  const note = s.note ?? (lab || old ? ago(p.start_ms) : undefined);
  return (
    <button type="button" className={`tile${old ? " stale" : ""}`} onClick={() => open(p.metric)}>
      <span className="tile-name">{nameOf(p.metric, p.name)}</span>
      <span className="tile-value">
        {s.value}
        {s.unit && <span className="unit"> {s.unit}</span>}
        <Flag p={p} />
      </span>
      <span className="tile-foot">
        {note && <span className="fine">{note}</span>}
        {!old && days && <Trend p={p} days={days} />}
      </span>
    </button>
  );
}

/** Key, everyday measures show their month's trend; lab and old values show their date instead. */
function AnyTile({ p, open, lab }: { p: Point; open: (code: string) => void; lab: boolean }) {
  if (!lab && !isStale(p) && p.agg !== "cumulative" && typeof p.value === "number") return <TrendTile p={p} open={open} s={shown(p)} />;
  return <Tile p={p} open={open} lab={lab} />;
}

/** How many tiles fit in one row of `.tiles` (176px minimum, 10px gaps; see styles.css). */
function useColumns(): [React.RefObject<HTMLDivElement>, number] {
  const ref = useRef<HTMLDivElement>(null);
  const [cols, setCols] = useState(4);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setCols(Math.max(1, Math.floor((e!.contentRect.width + 10) / (176 + 10)))));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, cols];
}

function Section({ g, rows, sources, open, all }: { g: (typeof GROUPS)[number]; rows: Point[]; sources: Source[]; open: (code: string) => void; all: boolean }) {
  const [more, setMore] = useState(false);
  const [grid, cols] = useColumns();
  const key = KEY[g.key] ?? [];
  // key measures first, then everyday ones, then detailed body-composition analysis, newest first within each
  const detailed = (p: Point) => /^x:(seca\/|fat_|ecw|extracellular|total_body_water|skeletal_muscle|appendicular|bia_|phase_angle|visceral|resting_energy|total_energy)/.test(p.metric);
  const ranked = [...rows].sort((a, b) => {
    const ka = key.indexOf(a.metric);
    const kb = key.indexOf(b.metric);
    if (ka >= 0 || kb >= 0) return (ka < 0 ? 99 : ka) - (kb < 0 ? 99 : kb);
    return Number(detailed(a)) - Number(detailed(b)) || b.start_ms - a.start_ms;
  });
  const lab = g.key === "lab" || g.sensitive;
  // collapsed: one full row (two for a category this big, so key measures and a few more show)
  const fold = cols * (rows.length > cols * 3 ? 2 : 1);
  const shownRows = all || more ? ranked : ranked.slice(0, fold);
  // where these numbers come from, newest first
  const bySource = new Map<number, number>();
  for (const p of rows) bySource.set(p.source_id, Math.max(bySource.get(p.source_id) ?? 0, p.start_ms));
  const from = [...bySource.entries()]
    .sort((a, b) => b[1] - a[1])
    .map(([id]) => sourceLabel(sources.find((x) => x.id === id)));
  return (
    <section className="section" id={`g-${g.key}`} aria-labelledby={`h-${g.key}`}>
      <header>
        <h3 id={`h-${g.key}`}>
          <GroupIcon k={g.key} />
          {g.label}
        </h3>
        {!all && rows.length > fold && (
          <button type="button" className="link" aria-expanded={more} onClick={() => setMore(!more)}>
            {more ? "Show fewer" : `Show all ${rows.length}`}
          </button>
        )}
      </header>
      <div className="tiles" ref={grid}>
        {shownRows.map((p) => (
          <AnyTile key={p.metric} p={p} open={open} lab={lab} />
        ))}
      </div>
      {from.length > 0 && (
        <p className="fine from">
          From {[...new Set(from)].slice(0, 3).join(", ")}
          {new Set(from).size > 3 ? ` and ${new Set(from).size - 3} more` : ""}
        </p>
      )}
    </section>
  );
}

/** Status pills, as Home's: the one headline number of each everyday category. */
function Summary({ latest, open }: { latest: Point[]; open: (code: string) => void }) {
  const picks = ["body", "cardio", "sleep", "activity"]
    .map((g) => (KEY[g] ?? []).map((k) => latest.find((p) => p.metric === k && typeof p.value === "number" && !isStale(p))).find(Boolean))
    .filter((p): p is Point => !!p);
  if (!picks.length) return null;
  return (
    <div className="pills" role="group" aria-label="At a glance">
      {picks.map((p) => (
        <Pill key={p.metric} p={p} open={open} />
      ))}
    </div>
  );
}

function Pill({ p, open }: { p: Point; open: (code: string) => void }) {
  return p.agg === "cumulative" ? <SummedPill p={p} open={open} /> : <PillBody p={p} open={open} s={shown(p)} />;
}

function SummedPill({ p, open }: { p: Point; open: (code: string) => void }) {
  return <PillBody p={p} open={open} s={display(p, useMonth(p))} />;
}

function PillBody({ p, open, s }: { p: Point; open: (code: string) => void; s: { value: string; unit: string } }) {
  return (
    <button type="button" className="pill" onClick={() => open(p.metric)}>
      <GroupIcon k={p.group} />
      <span>
        <span className="pill-name">{nameOf(p.metric, p.name)}</span>
        <span className="pill-value">
          {s.value}
          {s.unit && ` ${s.unit}`}
        </span>
      </span>
    </button>
  );
}

function Readings({
  latest,
  err,
  open,
  go,
  reviews,
  find,
  found,
}: {
  latest?: Point[];
  err?: string;
  open: (code: string) => void;
  go: (p: Page) => void;
  reviews: number;
  find: boolean;
  found: () => void;
}) {
  const search = useRef<HTMLInputElement>(null);
  const [sources] = useOp(() => op<Source[]>("source.list"), []);
  const [q, setQ] = useState("");
  useEffect(() => {
    if (find && search.current) {
      search.current.focus();
      found();
    }
  }, [find, latest, found]);
  if (err) return <Problem text={err} />;
  if (!latest) return <p className="empty" aria-busy="true">Loading your readings…</p>;
  if (!latest.length)
    return (
      <section className="page">
        <h2>Readings</h2>
        <p className="lede">
          Nothing here yet. Restore a Health backup (.zuhealth), import an Apple Health or WHOOP export, or add a reading by hand. You can also
          drop a file anywhere on this window.
        </p>
        <div className="actions">
          <button type="button" className="primary" onClick={() => go("add")}>
            Add data
          </button>
        </div>
      </section>
    );
  const needle = q.trim().toLowerCase();
  const match = (p: Point) => !needle || nameOf(p.metric, p.name).toLowerCase().includes(needle) || p.name.toLowerCase().includes(needle);
  const groups = GROUPS.map((g) => ({ g, rows: latest.filter((p) => p.group === g.key && match(p)) })).filter((x) => x.rows.length);
  const newest = Math.max(...latest.map((p) => p.start_ms));
  return (
    <section className="page wide">
      <div className="page-head">
        <div>
          <h2>Readings</h2>
          <p className="fine">Updated {when(newest)}</p>
        </div>
        <div className="search-field">
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
            <circle cx="7" cy="7" r="4.5" />
            <path d="M10.5 10.5L14 14" />
          </svg>
          <input
            ref={search}
            type="search"
            placeholder={`Find a measure (${MOD}F)`}
            aria-label="Find a measure"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setQ("")}
          />
        </div>
      </div>
      {!needle && <Summary latest={latest} open={open} />}
      {reviews > 0 && (
        <p className="notice">
          {reviews} {reviews === 1 ? "import is" : "imports are"} waiting for you to check.{" "}
          <button type="button" className="link" onClick={() => go("review")}>
            Review now
          </button>
        </p>
      )}
      {!groups.length && <p className="empty">No measure matches “{q}”.</p>}
      {groups.map(({ g, rows }) => (
        <Section key={g.key} g={g} rows={rows} sources={sources ?? []} open={open} all={!!needle} />
      ))}
    </section>
  );
}

const RANGES = [
  { label: "30 days", days: 30 },
  { label: "90 days", days: 90 },
  { label: "1 year", days: 365 },
  { label: "All", days: 0 },
];

function MetricPicker({ onPick, placeholder }: { onPick: (m: Metric) => void; placeholder: string }) {
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<Metric[]>([]);
  useEffect(() => {
    if (q.trim().length < 2) return setHits([]);
    const t = setTimeout(() => op<Metric[]>("metric.search", { q, limit: 8 }).then(setHits, () => setHits([])), 150);
    return () => clearTimeout(t);
  }, [q]);
  return (
    <div className="picker">
      <input aria-label={placeholder} placeholder={placeholder} value={q} onChange={(e) => setQ(e.target.value)} />
      {hits.length > 0 && (
        <ul className="hits">
          {hits.map((m) => (
            <li key={m.code}>
              <button
                type="button"
                onClick={() => {
                  onPick(m);
                  setQ("");
                }}
              >
                {m.name} <span className="fine">{m.code}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function Record({ code, pick }: { code?: string; pick: (c: string) => void }) {
  // the shortest range with enough days to show a shape; the person can still choose
  const [range, setRange] = useState<number | null>(null);
  const [info] = useOp(() => (code ? op<MetricInfo>("metric.show", { code }) : Promise.resolve(undefined)), [code]);
  const [allDays] = useOp(() => (code ? op<Day[]>("daily.query", { metric: code }) : Promise.resolve([] as Day[])), [code]);
  const [points] = useOp(() => (code ? op<Point[]>("obs.query", { metric: code, limit: 30, newest_first: true }) : Promise.resolve([])), [code]);
  const [sources] = useOp(() => op<Source[]>("source.list"), []);
  useEffect(() => setRange(null), [code]);
  const auto = useMemo(() => {
    const days = allDays ?? [];
    for (const r of [30, 90, 365]) if (days.filter((d) => dayMs(d.day) >= Date.now() - r * DAY).length >= 7) return r;
    return 0;
  }, [allDays]);
  const span = range ?? auto;
  const days = (allDays ?? []).filter((d) => !span || dayMs(d.day) >= Date.now() - span * DAY);
  const recent = points ?? [];
  if (!code)
    return (
      <section className="page">
        <h2>History</h2>
        <MetricPicker placeholder="Search measures, e.g. weight" onPick={(m) => pick(m.code)} />
      </section>
    );
  const agg = info?.agg ?? "instant";
  const pts = toPts(agg, days);
  const unit = dayUnit(agg, info?.unit ?? "");
  const vals = pts.map((p) => p.v);
  // highs and lows of every reading, not of daily means (heart rate: many readings a day)
  const lows = agg === "instant" ? days.map((d) => d.min).filter((v): v is number => v !== null) : vals;
  const highs = agg === "instant" ? days.map((d) => d.max).filter((v): v is number => v !== null) : vals;
  const lab = isLabGroup(recent[0]?.group);
  // durations read as hours and minutes everywhere: stats, axis, tooltip, table (interval days are in hours)
  // durations of a minute or more read as hours and minutes (as tiles do); HRV's 40 ms stays "40 ms"
  const big = (u: string) => vals.length > 0 && asDuration(Math.max(...vals.map(Math.abs)), u, true) !== undefined;
  const dur = agg === "interval" || ((agg === "instant" || agg === "cumulative") && PER_MS_UNITS.has(info?.unit ?? "") && big(info?.unit ?? ""));
  // lab: no more decimals than the reports printed (an average must not grow six of them)
  const places = Math.max(0, ...recent.map((p) => (typeof p.value === "number" ? (String(p.value).split(".")[1]?.length ?? 0) : 0)));
  const labNum = (v: number) => v.toLocaleString(undefined, { maximumFractionDigits: Math.min(places, 6) });
  const fmt = (v: number) =>
    !dur ? (lab ? labNum(v) : num(v)) : agg === "interval" ? (asDuration(v, "h") ?? num(v)) : (asDuration(v, info!.unit) ?? num(v));
  const shownUnit = dur ? "" : unit;
  const ref = recent.find((p) => p.detail?.ref || (p.detail?.printed as { ref?: string } | undefined)?.ref);
  const refText = (ref?.detail?.ref ?? (ref?.detail?.printed as { ref?: string } | undefined)?.ref) as string | undefined;
  const sourceName = (id: number) => {
    return sourceLabel(sources?.find((s) => s.id === id));
  };
  return (
    <section className="record">
      <h2>{info ? nameOf(info.code, info.name) : code}</h2>
      <p className="fine" title={code}>
        {info && nameOf(info.code, info.name) !== info.name ? info.name : code}
      </p>
      <div className="segmented" role="group" aria-label="Range">
        {RANGES.map((r) => (
          <button key={r.label} type="button" aria-pressed={span === r.days} onClick={() => setRange(r.days)}>
            {r.label}
          </button>
        ))}
      </div>
      {vals.length > 0 && (
        <dl className="stats">
          <div>
            <dt>Average</dt>
            <dd>
              {fmt(vals.reduce((a, b) => a + b, 0) / vals.length)} <span className="unit">{shownUnit}</span>
            </dd>
          </div>
          <div>
            <dt>Lowest</dt>
            <dd>
              {fmt(Math.min(...lows))} <span className="unit">{shownUnit}</span>
            </dd>
          </div>
          <div>
            <dt>Highest</dt>
            <dd>
              {fmt(Math.max(...highs))} <span className="unit">{shownUnit}</span>
            </dd>
          </div>
          <div>
            <dt>Days with data</dt>
            <dd>{vals.length}</dd>
          </div>
          {refText && (
            <div>
              <dt>Reference range</dt>
              <dd>{refText}</dd>
            </div>
          )}
        </dl>
      )}
      <DailyChart pts={pts} unit={shownUnit} fmt={fmt} />
      <h3>Latest entries</h3>
      <table className="rows">
        <tbody>
          {recent.map((p) => {
            const s = shown(p);
            return (
              <tr key={`${p.start_ms}-${p.source_id}`}>
                <td>{p.detail?.precision === "day" ? dateIn(p.start_ms, p.tz_min) : when(p.start_ms, p.tz_min)}</td>
                <td className="fine">{sourceName(p.source_id)}</td>
                <td className="n value">
                  {s.value} <span className="unit">{s.unit}</span>
                  <Flag p={p} />
                  {p.corrected && <span className="fine"> corrected</span>}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <MetricPicker placeholder="Look at another measure" onPick={(m) => pick(m.code)} />
    </section>
  );
}

/** Common measures, one tap away (as the web version's record page). */
const QUICK: [string, string][] = [
  ["loinc:29463-7", "Weight"],
  ["loinc:8480-6", "Blood pressure, systolic"],
  ["loinc:8462-4", "Blood pressure, diastolic"],
  ["loinc:8867-4", "Heart rate"],
  ["loinc:2339-0", "Blood glucose"],
  ["loinc:8310-5", "Body temperature"],
];

/** A typed number: "10,000" and "10 000" are ten thousand; "36,6" and "0,125" use a decimal comma; anything
 *  else that isn't a plain number is NaN. The form shows how it was read before saving (see `Enter`). */
export function parseNumber(s: string): number {
  const t = s.replace(/[\s\u00a0']/g, "");
  if (/^-?0\d*,\d+$/.test(t)) return Number(t.replace(",", ".")); // a leading 0 is never a thousands group
  if (/^-?\d{1,3}(,\d{3})+(\.\d+)?$/.test(t)) return Number(t.replaceAll(",", ""));
  if (/^-?\d+,\d+$/.test(t)) return Number(t.replace(",", "."));
  return /^-?(\d+\.?\d*|\.\d+)$/.test(t) ? Number(t) : Number.NaN;
}

function localNow() {
  const d = new Date();
  d.setMinutes(d.getMinutes() - d.getTimezoneOffset());
  return d.toISOString().slice(0, 16);
}

function Enter({ done }: { done: (code: string) => void }) {
  const [m, setM] = useState<MetricInfo>();
  const [value, setValue] = useState("");
  const [unit, setUnit] = useState("");
  const [at, setAt] = useState(localNow());
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  const pick = async (x: Metric) => {
    const info = await op<MetricInfo>("metric.show", { code: x.code });
    setM(info);
    setUnit(info.units.length === 1 ? (info.units[0] ?? "") : "");
    // a fresh form: now, no value, no old error
    setAt(localNow());
    setValue("");
    setErr(undefined);
  };
  const save = async () => {
    if (!m) return;
    const local = new Date(at);
    const text = m.agg === "none";
    const trimmed = value.trim();
    if (!trimmed) return setErr("Type a value.");
    const v = text ? trimmed : parseNumber(trimmed);
    if (!text && !Number.isFinite(v)) return setErr("Type a number, such as 72 or 36.6.");
    if (busy) return;
    setBusy(true);
    try {
      await op("manual.enter", {
        entries: [{ metric: m.code, value: v, ...(unit ? { unit } : {}), at: { ms: local.getTime(), tz_min: -local.getTimezoneOffset() } }],
      });
      done(m.code);
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="page narrow">
      <h2>Enter by hand</h2>
      {!m ? (
        <>
          <MetricPicker placeholder="What did you measure?" onPick={pick} />
          <div className="chips" role="group" aria-label="Common measures">
            {QUICK.map(([code, label]) => (
              <button key={code} type="button" className="chip-button" onClick={() => pick({ code, name: label, unit: "", agg: "instant" })}>
                {label}
              </button>
            ))}
          </div>
        </>
      ) : (
        <form
          className="form"
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          <p>
            <strong>{m.name}</strong>{" "}
            <button type="button" className="link" onClick={() => setM(undefined)}>
              Change
            </button>
          </p>
          <label>
            Value
            <input inputMode={m.agg === "none" ? "text" : "decimal"} value={value} onChange={(e) => setValue(e.target.value)} required />
          </label>
          {m.agg !== "none" && value.includes(",") && Number.isFinite(parseNumber(value)) && (
            <p className="fine" role="status">
              Read as {parseNumber(value).toLocaleString(undefined, { maximumFractionDigits: 6 })}
            </p>
          )}
          {m.units.length > 1 && (
            <label>
              Unit
              <select value={unit} onChange={(e) => setUnit(e.target.value)} required>
                <option value="">Choose the unit on your device</option>
                {m.units.map((u) => (
                  <option key={u} value={u}>
                    {unitLabel(u) || u}
                  </option>
                ))}
              </select>
            </label>
          )}
          <label>
            When
            <input type="datetime-local" value={at} onChange={(e) => setAt(e.target.value)} required />
          </label>
          <Problem text={err} />
          <button type="submit" className="primary" disabled={busy}>
            Save reading
          </button>
        </form>
      )}
    </section>
  );
}

const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

function Sources({ go }: { go: (p: Page) => void }) {
  const [sources, , reloadSources] = useOp(() => op<Source[]>("source.list"), []);
  const [batches, err, reload] = useOp(() => op<Batch[]>("import.list"), []);
  const [confirm, setConfirm] = useState<number>();
  const [msg, setMsg] = useState<string>();
  const [ordering, setOrdering] = useState(false);
  const run = async (fn: () => Promise<unknown>, ok: string) => {
    setMsg(undefined);
    try {
      await fn();
      setMsg(ok);
      reload();
      reloadSources();
    } catch (e) {
      setMsg(errText(e));
    }
  };
  /** Moves a source one place up by swapping it with the one above (ties included), then numbers them 10, 20, 30… */
  const moveUp = async (i: number) => {
    setOrdering(true);
    await run(async () => {
      const order = [...(sources ?? [])];
      [order[i - 1], order[i]] = [order[i]!, order[i - 1]!];
      for (const [k, s] of order.entries()) if (s.priority !== (k + 1) * 10) await op("source.update", { id: s.id, priority: (k + 1) * 10 });
    }, "Order saved.");
    setOrdering(false);
  };
  const device = (id: number) => {
    const x = sources?.find((s) => s.id === id);
    // inside a kind, repeating a generic device name ("OCR") says nothing
    return !x || x.connector === "manual" || sourceLabel(x) === sourceKind(x.connector) ? "" : `${x.name} · `;
  };
  // one row per kind of source, newest first, opening its imports
  const kinds = new Map<string, Batch[]>();
  for (const b of batches ?? []) kinds.set(b.connector, [...(kinds.get(b.connector) ?? []), b]);
  const ordered = [...kinds.entries()].sort((a, b) => Math.max(...b[1].map((x) => x.at ?? 0)) - Math.max(...a[1].map((x) => x.at ?? 0)));
  return (
    <section className="page narrow">
      <h2>Sources</h2>
      <p className="lede">
        Where your readings came from. Open a kind to see each import, or delete one: its readings go with it.{" "}
        <button type="button" className="link" onClick={() => go("add")}>
          Add data
        </button>
      </p>
      {msg && (
        <p className="notice" role="status">
          {msg}
        </p>
      )}
      <Problem text={err} />
      {batches && !batches.length && <p className="empty">No imports yet.</p>}
      <div className="list-card">
        {ordered.map(([kind, list]) => {
          const devices = new Set(list.map((b) => b.source_id));
          const last = Math.max(...list.map((b) => b.at ?? 0));
          return (
            <details key={kind} className="kind">
              <summary>
                <span className="name">{sourceKind(kind)}</span>
                <span className="fine">
                  {plural(list.length, "import")}
                  {devices.size > 1 ? `, ${plural(devices.size, "device")}` : ""} · {last ? shortDate(last) : ""}
                </span>
              </summary>
              <ul className="batches">
                {list.map((b) => (
                  <li key={b.import_id}>
                    <span>
                      {b.at ? when(b.at) : ""}
                      <br />
                      <span className="fine">
                        {device(b.source_id)}
                        {b.status === "review" ? <span className="signal">waiting for you</span> : plural(b.metrics, "measure")}
                      </span>
                    </span>
                    {confirm === b.import_id ? (
                      <span className="actions">
                        <button type="button" className="danger" onClick={() => run(() => op("import.erase", { id: b.import_id }), "Deleted, with its readings.")}>
                          Delete import and its readings
                        </button>
                        <button type="button" onClick={() => setConfirm(undefined)}>
                          Keep
                        </button>
                      </span>
                    ) : (
                      <button type="button" className="link" onClick={() => setConfirm(b.import_id)}>
                        Delete…
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </details>
          );
        })}
      </div>
      <details className="group">
        <summary>
          <h3>When two sources report the same day</h3>
        </summary>
        <p className="fine">The one higher in this list is used for daily summaries. Nothing is added up.</p>
        <ol className="order">
          {(sources ?? []).map((s, i) => (
            <li key={s.id}>
              <span>
                {s.connector === "manual" ? "Entered by you" : s.name} <span className="fine">{sourceKind(s.connector)}</span>
              </span>
              <button type="button" className="link" disabled={i === 0 || ordering} aria-label={`Prefer ${s.name}`} onClick={() => moveUp(i)}>
                Move up
              </button>
            </li>
          ))}
        </ol>
      </details>
    </section>
  );
}

function Review() {
  const [items, err, reload] = useOp(() => op<BatchDetail[]>("review.list"), []);
  const [grants] = useOp(() => op<Grant[]>("grant.list"), []);
  const [msg, setMsg] = useState<{ ok: boolean; text: string }>();
  const act = async (name: string, id: number, ok: string) => {
    try {
      await op(name, { id });
      setMsg({ ok: true, text: ok });
      reload();
    } catch (e) {
      setMsg({ ok: false, text: errText(e) });
    }
  };
  // which agent proposed it: its writer key (older proposals: "g<grant id>")
  const agent = (by?: unknown) => grants?.find((g) => g.writer === by || `g${g.id}` === by)?.name;
  if (err) return <Problem text={err} />;
  return (
    <section className="page">
      <h2>Waiting for you</h2>
      <p className="lede">Nothing here is visible in your readings until you confirm it.</p>
      {msg &&
        (msg.ok ? (
          <p className="fine" role="status">
            {msg.text}
          </p>
        ) : (
          <Problem text={msg.text} />
        ))}
      {items && !items.length && <p className="empty">Nothing is waiting.</p>}
      {(items ?? []).map((b) => {
        const proposal = b.candidates?.kind === "proposal";
        const rows = (b.candidates?.rows ?? []) as { metric: string; start_ms: number; tz_min?: number; value: unknown; unit?: string }[];
        const by = proposal ? agent(b.candidates?.by) : undefined;
        return (
          <div key={b.import_id} className="group">
            <h3>{proposal ? `Proposed by ${by ?? "an agent"}` : "From a lab report photo"}</h3>
            {b.candidates?.note && <p>“{b.candidates.note}”</p>}
            {proposal ? (
              <table className="rows">
                <tbody>
                  {rows.map((r) => (
                    <tr key={`${r.metric}-${r.start_ms}`}>
                      <td title={r.metric}>{nameOf(r.metric, r.metric)}</td>
                      <td>{when(r.start_ms, r.tz_min)}</td>
                      <td className="n">
                        {String(r.value)} <span className="unit">{unitLabel(r.unit ?? "")}</span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : (
              <p className="fine">Matching lab items to measures arrives with photo import.</p>
            )}
            <div className="actions">
              {proposal && (
                <button type="button" className="primary" onClick={() => act("review.confirm", b.import_id, "Added to your readings.")}>
                  Add to my readings
                </button>
              )}
              <button type="button" onClick={() => act("review.discard", b.import_id, "Discarded.")}>
                Discard
              </button>
            </div>
          </div>
        );
      })}
    </section>
  );
}

const windowText = (w: Grant["window"]) => (w === "all" ? "all time" : "last_days" in w ? `last ${w.last_days} days` : "a date range");
const writeText = { none: "read only", propose: "may propose readings", derived: "may propose and write computed results" };

const EXPIRY = [
  [30, "30 days"],
  [90, "90 days"],
  [365, "1 year"],
] as const;

/** After a grant is made: how to plug it into the chosen app, one click where the shell can do it. */
function ConnectSteps({ client, token, bin, found, done }: { client: Client; token: string; bin: string; found: boolean; done: () => void }) {
  const [result, setResult] = useState<{ ok: boolean; text: string }>();
  const [busy, setBusy] = useState(false);
  const oneClick = client.oneClick && isDesktop;
  return (
    <div className="group notice connect" role="status">
      <h3>Connect {client.name}</h3>
      {oneClick && (
        <>
          <p>
            {found ? `${client.name} is on this computer.` : `${client.name}'s settings folder wasn't found; it is created if you go ahead.`} Health
            can add itself to its settings for you. Other servers there are kept, and the original file is saved next to it (.health-bak).
          </p>
          <div className="actions">
            <button
              type="button"
              className="primary"
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                try {
                  const f = await connectAgent(client.id, token);
                  setResult({ ok: true, text: `Added to ${f}. Restart ${client.name} to pick it up.` });
                } catch (e) {
                  setResult({ ok: false, text: String(e) });
                }
                setBusy(false);
              }}
            >
              Add to {client.name}
            </button>
          </div>
          {result && <p className={result.ok ? "fine" : "problem"}>{result.text}</p>}
          <p className="fine">Or do it by hand:</p>
        </>
      )}
      <ol className="steps">
        {client.steps(bin, token).map((st) => (
          <li key={st.text}>
            {st.text}
            {st.code && (
              <div className="snippet">
                <pre>{st.code}</pre>
                <Copy text={st.code} label="Copy" />
              </div>
            )}
          </li>
        ))}
      </ol>
      <p className="fine">
        The token is shown only this once. The app works with your data even when Health is closed (with a passphrase lock, keep Health open and
        unlocked). If you move the Health app, or the access expires, connect again. Apps that run in a web browser (such as
        ChatGPT or claude.ai) can't reach Health: it only answers on this computer.
      </p>
      <button type="button" className="link" onClick={done}>
        Done
      </button>
    </div>
  );
}

/** Revoking cuts the app off at once: ask first, and say so if it fails. */
function Revoke({ id, name, done }: { id: number; name: string; done: () => void }) {
  const [ask, setAsk] = useState(false);
  const [err, setErr] = useState<string>();
  if (!ask)
    return (
      <button type="button" className="link" onClick={() => setAsk(true)}>
        Revoke
      </button>
    );
  return (
    <span className="actions inline">
      <button
        type="button"
        className="danger"
        onClick={() =>
          op("grant.revoke", { id }).then(
            () => (setAsk(false), done()),
            (e) => setErr(errText(e)),
          )
        }
      >
        Revoke {name}
      </button>
      <button type="button" onClick={() => setAsk(false)}>
        Keep
      </button>
      {err && <Problem text={err} />}
    </span>
  );
}

function LastUsed({ id }: { id: number }) {
  const [last, , reload] = useOp(() => op<Audit[]>("audit.list", { grant_id: id, limit: 1 }), [id]);
  usePoll(reload);
  if (!last) return null;
  return <span className="fine">{last[0] ? `last used ${ago(last[0].at)}` : "not used yet"}</span>;
}

function Agents() {
  const [grants, err, reload] = useOp(() => op<Grant[]>("grant.list"), []);
  const [found, setFound] = useState<string[]>([]);
  const [client, setClient] = useState<Client>();
  const [name, setName] = useState("");
  const [groups, setGroups] = useState<string[]>(GROUPS.filter((g) => !g.sensitive).map((g) => g.key));
  const [detail, setDetail] = useState(false);
  const [days, setDays] = useState(90);
  const [expiry, setExpiry] = useState(90);
  const [write, setWrite] = useState<Grant["write"]>("none");
  const [reports, setReports] = useState(false);
  const [made, setMade] = useState<{ client: Client; token: string }>();
  const [msg, setMsg] = useState<string>();
  const [bin, setBin] = useState("health");
  useEffect(() => {
    corePath().then(setBin, () => {});
    agentClients().then((c) => setFound(c.map((x) => x.id)), () => {});
  }, []);
  const pick = (c: Client) => {
    setClient(c);
    setName(c.id === "other" ? "" : c.name);
    setMade(undefined);
  };
  const create = async () => {
    if (!client) return;
    setMsg(undefined);
    try {
      const r = await op<Grant & { token: string }>("grant.create", {
        name,
        groups: write === "derived" ? [...new Set([...groups, "derived"])] : groups,
        detail,
        reports,
        window: days ? { last_days: days } : "all",
        write,
        expires_ms: Date.now() + expiry * DAY,
      });
      setMade({ client, token: r.token });
      setClient(undefined);
      reload();
    } catch (e) {
      setMsg(errText(e));
    }
  };
  const toggle = (k: string) => setGroups((g) => (g.includes(k) ? g.filter((x) => x !== k) : [...g, k]));
  const ordered = [...CLIENTS].sort((a, b) => Number(found.includes(b.id)) - Number(found.includes(a.id)));
  return (
    <section className="page">
      <h2>Agent access</h2>
      <p className="lede">
        Let an AI app (Claude, Codex, Cursor, Copilot, Gemini and others) read your data through MCP. It sees only what you allow here, can
        never delete anything or read your backups, and anything it writes waits for you or lands under its own computed measures.
      </p>
      {made && <ConnectSteps client={made.client} token={made.token} bin={bin} found={found.includes(made.client.id)} done={() => setMade(undefined)} />}
      {!made && (
        <div className="group">
          <h3>Connect an app</h3>
          <div className="clients" role="group" aria-label="Agent apps">
            {ordered.map((c) => (
              <button key={c.id} type="button" className="client" aria-pressed={client?.id === c.id} onClick={() => pick(c)}>
                {c.name}
                {found.includes(c.id) && <span className="fine">On this computer</span>}
              </button>
            ))}
          </div>
        </div>
      )}
      {client && !made && (
        <form
          className="group form"
          onSubmit={(e) => {
            e.preventDefault();
            create();
          }}
        >
          <h3>What {client.id === "other" ? "the app" : client.name} may see</h3>
          <label>
            Name shown in the access log
            <input placeholder="My agent" value={name} onChange={(e) => setName(e.target.value)} required />
          </label>
          {[false, true].map((sensitive) => (
            <fieldset key={String(sensitive)}>
              <legend>{sensitive ? "Sensitive: tick only what the agent needs" : "What it may read"}</legend>
              {GROUPS.filter((g) => g.key !== "derived" && g.sensitive === sensitive).map((g) => (
                <label key={g.key} className="check">
                  <input type="checkbox" checked={groups.includes(g.key)} onChange={() => toggle(g.key)} />
                  {g.label}
                </label>
              ))}
            </fieldset>
          ))}
          <label className="check">
            <input type="checkbox" checked={detail} onChange={(e) => setDetail(e.target.checked)} />
            Every reading (otherwise daily summaries only)
          </label>
          <label className="check">
            <input type="checkbox" checked={reports} onChange={(e) => setReports(e.target.checked)} />
            Full text of lab reports (shows every item on the sheet, whatever is ticked above)
          </label>
          <label>
            How far back
            <select value={days} onChange={(e) => setDays(Number(e.target.value))}>
              <option value={30}>30 days</option>
              <option value={90}>90 days</option>
              <option value={365}>1 year</option>
              <option value={0}>Everything</option>
            </select>
          </label>
          <label>
            Writing
            <select value={write} onChange={(e) => setWrite(e.target.value as Grant["write"])}>
              <option value="none">Read only</option>
              <option value="propose">May propose readings for me to confirm</option>
              <option value="derived">May also write computed results</option>
            </select>
          </label>
          <label>
            Access ends after
            <select value={expiry} onChange={(e) => setExpiry(Number(e.target.value))}>
              {EXPIRY.map(([d, l]) => (
                <option key={d} value={d}>
                  {l}
                </option>
              ))}
            </select>
          </label>
          <Problem text={msg} />
          <div className="actions">
            <button type="submit" className="primary" disabled={!groups.length}>
              Create access
            </button>
            <button type="button" onClick={() => setClient(undefined)}>
              Cancel
            </button>
          </div>
        </form>
      )}
      <div className="group">
        <h3>Current access</h3>
        <Problem text={err} />
        {grants && !grants.length && <p className="empty">No app has access yet.</p>}
        <table className="rows">
          <tbody>
            {(grants ?? [])
              .slice()
              .reverse()
              .map((g) => {
                const live = !g.revoked_ms && g.expires_ms > Date.now();
                return (
                  <tr key={g.id} className={live ? "" : "stale"}>
                    <th scope="row">
                      {g.name}
                      <br />
                      <LastUsed id={g.id} />
                    </th>
                    <td>
                      {g.groups.map(groupLabel).join(", ")}; {g.detail ? "every reading" : "daily summaries"}, {windowText(g.window)}, {writeText[g.write]}
                    </td>
                    <td>{live ? `until ${new Date(g.expires_ms).toLocaleDateString()}` : g.revoked_ms ? "revoked" : "expired"}</td>
                    <td className="n">
                      {live && <Revoke id={g.id} name={g.name} done={reload} />}
                    </td>
                  </tr>
                );
              })}
          </tbody>
        </table>
      </div>
    </section>
  );
}

/** Agents' reads change no data, so no change event announces them: look again every few seconds. */
function usePoll(reload: () => void, ms = 5000) {
  useEffect(() => {
    const t = setInterval(reload, ms);
    return () => clearInterval(t);
  }, [reload, ms]);
}

function AuditLog() {
  const [log, err, reloadLog] = useOp(() => op<Audit[]>("audit.list", { limit: 200 }), []);
  usePoll(reloadLog);
  const [grants] = useOp(() => op<Grant[]>("grant.list"), []);
  const nameOf = (id: number) => grants?.find((g) => g.id === id)?.name ?? `#${id}`;
  return (
    <section className="page">
      <h2>Access log</h2>
      <p className="lede">Every call an agent made: what it asked for and how many rows it got back. Values are never logged.</p>
      <Problem text={err} />
      {log && !log.length && <p className="empty">No agent has read anything yet.</p>}
      <table className="rows">
        <tbody>
          {(log ?? []).map((a, i) => (
            <tr key={`${a.at}-${i}`}>
              <td>{when(a.at)}</td>
              <th scope="row">{nameOf(a.grant_id)}</th>
              <td>{a.op}</td>
              <td>{a.metrics.join(", ")}</td>
              <td className="n">{a.denied ? <span className="signal">refused</span> : `${a.rows} rows`}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

function Backup({ go }: { go: (p: Page) => void }) {
  const [out, setOut] = useState("");
  const [pw, setPw] = useState("");
  const [pw2, setPw2] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();
  const [done, setDone] = useState<string>();
  // local date and time: a second backup the same day gets its own name
  const stamp = (() => {
    const d = new Date();
    const p2 = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}${p2(d.getMonth() + 1)}${p2(d.getDate())}-${p2(d.getHours())}${p2(d.getMinutes())}`;
  })();
  const save = async () => {
    setErr(undefined);
    setDone(undefined);
    if (pw.length < 8) return setErr("Use at least 8 characters.");
    if (pw !== pw2) return setErr("The two passwords differ.");
    let path = out;
    if (isDesktop) {
      const p = await chooseSavePath(`health-${stamp}.zuhealth`, "Health backup", "zuhealth");
      if (!p) return;
      path = p;
    }
    if (!path) return setErr("Choose where to save the backup.");
    setBusy(true);
    try {
      // the native save dialog already asked before replacing a file
      const r = await op<{ bytes: number; path: string }>("backup.export", { path, password: pw, replace: isDesktop });
      setDone(`Saved ${fileName(r.path)} (${(r.bytes / 1e6).toFixed(1)} MB). Keep the password safe: without it the backup cannot be opened.`);
      setPw("");
      setPw2("");
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="page narrow">
      <h2>Backup</h2>
      <p className="lede">
        A backup is one encrypted file with everything in Health. It also opens in the web version. To restore one,{" "}
        <button type="button" className="link" onClick={() => go("add")}>
          import it
        </button>
        .
      </p>
      <form
        className="group form"
        aria-busy={busy}
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <PasswordField label="Password" value={pw} onChange={setPw} auto="new-password" />
        <PasswordField label="Repeat password" value={pw2} onChange={setPw2} auto="new-password" />
        <p className="fine">At least 8 characters. There is no way to recover a forgotten backup password.</p>
        {!isDesktop && (
          <label>
            Save to
            <input value={out} onChange={(e) => setOut(e.target.value)} placeholder={`/Users/you/Documents/health-${stamp}.zuhealth`} />
          </label>
        )}
        <Problem text={err} />
        <button type="submit" className="primary" disabled={busy}>
          {busy ? "Saving…" : isDesktop ? "Save backup…" : "Save backup"}
        </button>
      </form>
      {done && (
        <p className="notice" role="status">
          {done}
        </p>
      )}
    </section>
  );
}
