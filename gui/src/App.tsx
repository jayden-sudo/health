import { type ReactNode, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  type Audit,
  type Batch,
  type BatchDetail,
  type Day,
  type Grant,
  type Metric,
  type MetricInfo,
  type OcrItem,
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
  installOcr,
  isDesktop,
  menuOwnsKeys,
  onFileDrop,
  onMenu,
  setBadge,
  onStatus,
  op,
  setShellLanguage,
  unlock,
} from "./api";
import { DailyChart, type Pt, Spark } from "./charts";
import { useTranslation } from "react-i18next";
import { LANGS, choice, choose, locale as i18nLocale, t, tk } from "./i18n";
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

type Page = "readings" | "data" | "backup" | "profile" | "account" | "agents" | "review";
/** The sections, in shortcut order (⌘1…⌘6). */
const PAGES: { key: Page; label: string }[] = [
  { key: "readings", label: tk("Readings") },
  { key: "data", label: tk("Data sources") },
  { key: "backup", label: tk("Backup") },
  { key: "profile", label: tk("Profile") },
  { key: "account", label: tk("Health account") },
  { key: "agents", label: tk("Agent access") },
];
const MAC = typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent);
const MOD = MAC ? "⌘" : "Ctrl+";

/** Sidebar icons: 16px outline, drawn in currentColor. */
const ICON: Record<string, ReactNode> = {
  readings: <path d="M2 12h3l2-6 3 9 2-5h2" />,
  data: (
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
  profile: (
    <>
      <rect x="2.5" y="3" width="11" height="10" rx="2" />
      <circle cx="6" cy="7" r="1.5" />
      <path d="M3.8 11c.4-1 1.2-1.6 2.2-1.6s1.8.6 2.2 1.6M9.5 6.5h2.5M9.5 9h2" />
    </>
  ),
  account: (
    <>
      <circle cx="8" cy="5.5" r="2.5" />
      <path d="M3 13.5c.8-2.4 2.7-3.5 5-3.5s4.2 1.1 5 3.5" />
    </>
  ),
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
    const p = localStorage.getItem("page");
    // before data sources were one page
    if (p === "add" || p === "sources") return "data";
    return p && PAGES.some((x) => x.key === p) ? (p as Page) : "readings";
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
      <h2>{t("Welcome")}</h2>
      <p className="lede">
        {t("Your health data will live in one encrypted file on this computer. The key is kept in your system keychain. Nothing is sent anywhere.")}
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
        {busy ? t("Creating…") : t("Create my health store")}
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
            setErr(x instanceof OpError && x.code === "bad_passphrase" ? t("That passphrase is not right.") : String(x));
          }
        }}
      >
        <h2>{t("Health is locked")}</h2>
        <p className="lede">{t("Your data is encrypted with your passphrase. It locks again after a while without use.")}</p>
        <label>
          {t("Passphrase")}
          <input type="password" autoComplete="current-password" autoFocus value={p} onChange={(e) => setP(e.target.value)} required />
        </label>
        <Problem text={err} />
        <button type="submit" className="primary">
          {t("Unlock")}
        </button>
      </form>
    </main>
  );
}

export function App() {
  // redraw everything when the language changes; the menu bar follows
  const { i18n } = useTranslation();
  useEffect(() => setShellLanguage(i18n.language), [i18n.language]);
  const [page, setPage] = useState<Page>(savedPage);
  // ⌘F asks Readings to focus its search; Readings clears the request once its search box exists
  const [find, setFind] = useState(false);
  useEffect(() => {
    try {
      if (PAGES.some((x) => x.key === page)) localStorage.setItem("page", page);
    } catch {}
  }, [page]);
  const [metric, setMetric] = useState<string>();
  useInbox();
  const [status, statusErr, reload] = useOp(() => op<Status>("status"), []);
  const [latest, latestErr] = useOp(() => op<Point[]>("obs.latest"), []);
  const [cats, setCats] = useState(true);
  // the category scrolled into view on Readings (null: its top)
  const [inView, setInView] = useState<string | null>(null);
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
            setPage("data");
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
    else if (id === "add") setPage("data");
    else if (id === "back") setMetric(undefined);
    else if (id === "find") {
      setPage("readings");
      setFind(true);
    } else if (id === "import")
      chooseImportFile().then((f) => {
        if (f) {
          setDropped(f);
          setPage("data");
        }
      });
  }, []);
  useEffect(() => onMenu(command), [command]);
  useEffect(() => {
    if (menuOwnsKeys) return;
    const keys: Record<string, string> = Object.fromEntries([
      ...PAGES.map((p, i) => [String(i + 1), p.key]),
      ["f", "find"],
      ["o", "import"],
      ["[", "back"],
    ]);
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
        <h2>{t("Starting the health core…")}</h2>
        <p className="lede">{core.error ?? t("This takes a moment. Your data is safe on disk.")}</p>
      </main>
    );
  const open = (code: string) => setMetric(code);
  const present = GROUPS.filter((g) => latest?.some((p) => p.group === g.key));
  const navItem = (p: { key: Page; label: string }, hint?: string, quiet = false) => (
    <li key={p.key}>
      <button
        type="button"
        className={page === p.key && !quiet ? "on" : ""}
        aria-current={page === p.key ? "page" : undefined}
        onClick={() => setPage(p.key)}
        title={hint}
      >
        <Icon k={p.key} />
        <span className="label">{t(p.label)}</span>
        {p.key === "review" && status && status.reviews > 0 && <span className="count">{status.reviews}</span>}
      </button>
    </li>
  );
  const hint = (k: Page) => {
    const i = PAGES.findIndex((x) => x.key === k);
    return `${t(PAGES[i]!.label)} (${MOD}${i + 1})`;
  };
  return (
    <div className={`app${isDesktop ? " desktop" : ""}${isDesktop && MAC ? " mac" : ""}`}>
      <div className="backdrop" aria-hidden="true" />
      {isDesktop && MAC && <div className="titlebar" data-tauri-drag-region />}
      {hovering && (
        <div className="drop-overlay" aria-hidden="true">
          <p>{t("Drop to import")}</p>
          <p className="fine">{t("Apple Health export, Health backup, WHOOP data")}</p>
        </div>
      )}
      <nav className="rail" aria-label={t("Sections")}>
        <ul>
          {navItem(PAGES[0]!, hint("readings"), page === "readings" && !!inView)}
          {status && status.reviews > 0 && navItem({ key: "review", label: tk("Waiting for you") })}
        </ul>
        {present.length > 0 && (
          <div className="rail-section">
            <button type="button" className="rail-head" aria-expanded={cats} onClick={() => setCats(!cats)}>
              {t("Categories")}
              <Chevron open={cats} />
            </button>
            {cats && (
              <ul>
                {present.map((g) => (
                  <li key={g.key}>
                    <button
                      type="button"
                      className={page === "readings" && inView === g.key ? "on" : ""}
                      aria-current={page === "readings" && inView === g.key ? "location" : undefined}
                      onClick={() => {
                        setPage("readings");
                        // marked at once: a short page can't scroll the section to the top
                        requestAnimationFrame(() => {
                          document.getElementById(`g-${g.key}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
                          setInView(g.key);
                        });
                      }}
                    >
                      <GroupIcon k={g.key} />
                      <span className="label">{t(g.label)}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
        <div className="rail-section">
          <p className="rail-head">{t("Your data")}</p>
          <ul>
            {navItem(PAGES[1]!, hint("data"))}
            {navItem(PAGES[2]!, hint("backup"))}
          </ul>
        </div>
        <div className="rail-section">
          <p className="rail-head">{t("Me")}</p>
          <ul>
            {navItem(PAGES[3]!, hint("profile"))}
            {navItem(PAGES[4]!, hint("account"))}
          </ul>
        </div>
        <div className="rail-section">
          <p className="rail-head">{t("Sharing")}</p>
          <ul>{navItem(PAGES[5]!, hint("agents"))}</ul>
        </div>
        <div className="rail-foot">
          {status && (
            <p className="fine">
              {t("{{obs}} readings from {{count}} sources, stored only on this computer", {
                obs: status.observations.toLocaleString(i18n.language),
                count: status.sources,
              })}
            </p>
          )}
          <label className="fine lang">
            {t("Language")}
            <select value={choice()} onChange={(e) => choose(e.target.value as Parameters<typeof choose>[0])}>
              {LANGS.map((l) => (
                <option key={l.key} value={l.key}>
                  {l.key === "system" ? t("System") : l.label}
                </option>
              ))}
            </select>
          </label>
        </div>
      </nav>
      <main className="main">
        <div className="view" key={`${page}-${i18n.language}`}>
          {page === "readings" && (
            <Readings
              latest={latest}
              err={latestErr}
              open={open}
              go={setPage}
              reviews={status?.reviews ?? 0}
              find={find}
              found={() => setFind(false)}
              onSection={setInView}
            />
          )}
          {page === "data" && <DataSources file={dropped} onFile={setDropped} go={setPage} done={open} />}
          {page === "review" && <Review />}
          {page === "agents" && <Agents />}
          {page === "backup" && <Backup />}
          {page === "profile" && <ProfilePage />}
          {page === "account" && <HealthAccount go={setPage} />}
        </div>
      </main>
      {metric && <Sheet label={t("History")} close={() => setMetric(undefined)}>
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
  // read during the first render, before an autoFocus field inside takes focus
  const [back] = useState(() => document.activeElement as HTMLElement | null);
  useLayoutEffect(() => {
    const d = ref.current;
    if (d && !d.open) {
      // showModal() focuses the close button, which WebKit rings as if tabbed to; focus the sheet itself,
      // unless a field inside already took focus (autoFocus runs before this effect)
      const now = document.activeElement as HTMLElement | null;
      const field = now && d.contains(now) ? now : null;
      d.showModal();
      (field ?? d).focus();
    }
    return () => {
      d?.close();
      back?.focus?.();
    };
  }, [back]);
  return (
    <dialog
      ref={ref}
      className="sheet"
      tabIndex={-1}
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
        <button type="button" className="close" aria-label={t("Close")} onClick={close}>
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
      {done ? t("Copied") : label}
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
        <button type="button" className="link" aria-label={show ? t("Hide {{field}}", { field: label }) : t("Show {{field}}", { field: label })} aria-pressed={show} onClick={() => setShow(!show)}>
          {show ? t("Hide") : t("Show")}
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
        {t("File path")}
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
        <p>{t("Drag a file here, or anywhere in this window.")}</p>
      )}
      <button
        type="button"
        className={path ? "" : "primary"}
        onClick={async () => {
          const p = await chooseImportFile();
          if (p) onPath(p);
        }}
      >
        {path ? t("Choose another file…") : t("Choose file…")}
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

/** A row of an inset grouped list: icon, name with a line under it, a live status, a chevron. */
function Row({ icon, label, sub, value, onClick }: { icon: ReactNode; label: string; sub?: string; value?: ReactNode; onClick: () => void }) {
  return (
    <li>
      <button type="button" className="row-btn" onClick={onClick}>
        {icon}
        <span className="row-l">
          <strong>{label}</strong>
          {sub && <span className="fine">{sub}</span>}
        </span>
        {value !== undefined && <span className="row-v fine">{value}</span>}
        <span className="row-chev" aria-hidden="true">
          ›
        </span>
      </button>
    </li>
  );
}

type SourceSheet = "manage" | "whoop" | "apple" | "report" | "lab" | "restore" | "manual" | "file";
/** Data sources (as the web's 更多 › 数据来源): managing what is here first, then one row per way data comes
 *  in, each with its live status; the steps open in a sheet. A file dropped on the window opens here. */
function DataSources({ file, onFile, go, done }: { file?: string; onFile: (p?: string) => void; go: (p: Page) => void; done: (code: string) => void }) {
  const [sheet, setSheet] = useState<SourceSheet>();
  const [batches] = useOp(() => op<Batch[]>("import.list"), []);
  const [whoop] = useOp(() => op<WhoopStatus>("whoop.status"), []);
  useEffect(() => {
    if (file) setSheet((s) => s ?? "file");
  }, [file]);
  const close = () => {
    setSheet(undefined);
    onFile(undefined);
  };
  const lastOf = (connector: string) => {
    const at = Math.max(0, ...(batches ?? []).filter((b) => b.connector === connector).map((b) => b.at ?? 0));
    return at ? t("Last import {{when}}", { when: shortDate(at) }) : batches ? t("Not imported yet") : undefined;
  };
  const whoopValue =
    whoop?.state === "connected"
      ? whoop.last_sync
        ? t("Synced {{when}}", { when: ago(whoop.last_sync) })
        : t("Connected")
      : whoop?.state === "reauth"
        ? t("Sign in again")
        : t("Not connected");
  const titles: Record<SourceSheet, string> = {
    manage: t("Manage data"),
    whoop: "WHOOP",
    apple: t("Apple Health"),
    report: t("Lab report"),
    lab: t("Lab self-service"),
    restore: t("Restore a backup"),
    manual: t("Enter by hand"),
    file: t("Import a file"),
  };
  return (
    <section className="page">
      <h2>{t("Data sources")}</h2>
      <p className="lede">{t("Where your readings come from. Every source stays on this computer; you can drop a file anywhere on the window.")}</p>
      <ul className="rows-card">
        <Row
          icon={<Icon k="data" />}
          label={t("Manage data")}
          sub={t("See and delete imports")}
          value={batches ? t("{{count}} imports", { count: batches.length }) : undefined}
          onClick={() => setSheet("manage")}
        />
      </ul>
      <ul className="rows-card">
        <Row icon={<GroupIcon k="cardio" />} label="WHOOP" sub={t("Syncs every hour while Health is open")} value={whoopValue} onClick={() => setSheet("whoop")} />
        <Row icon={<GroupIcon k="activity" />} label={t("Apple Health")} sub={t("Export from your iPhone")} value={lastOf("apple_health")} onClick={() => setSheet("apple")} />
        <Row icon={<GroupIcon k="lab" />} label={t("Lab report")} sub={t("A photo or a PDF")} value={lastOf("ocr")} onClick={() => setSheet("report")} />
        <Row icon={<GroupIcon k="infectious" />} label={t("Lab self-service")} sub={t("A code for a lab device")} onClick={() => setSheet("lab")} />
        <Row icon={<Icon k="backup" />} label={t("Restore a backup")} sub={t("A .zuhealth file")} onClick={() => setSheet("restore")} />
        <Row icon={<GroupIcon k="body" />} label={t("Enter by hand")} sub={t("Write down a reading")} value={lastOf("manual")} onClick={() => setSheet("manual")} />
      </ul>
      {sheet && (
        <Sheet label={titles[sheet]} close={close}>
          {sheet === "manage" && <Sources go={go} />}
          {sheet === "whoop" && <WhoopConnect />}
          {(sheet === "apple" || sheet === "report" || sheet === "restore" || sheet === "file") && (
            <Import file={file} onFile={onFile} go={go} kind={sheet} title={titles[sheet]} />
          )}
          {sheet === "lab" && <SelfService go={go} />}
          {sheet === "manual" && (
            <Enter
              done={(code) => {
                close();
                done(code);
              }}
            />
          )}
        </Sheet>
      )}
    </section>
  );
}

/** A self-service code for a lab device (protocol 3.3): the device id is beside its QR. */
function SelfService({ go }: { go: (p: Page) => void }) {
  const [device, setDevice] = useState("");
  const [code, setCode] = useState<{ code: string; until: number }>();
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  return (
    <section className="record">
      <h2>{t("Lab self-service")}</h2>
      <p className="lede">{t("At a lab device that offers self-service, type the code it asks for; the result is sent to your health account.")}</p>
      {code ? (
        <div className="group form">
          <p className="self-code" aria-label={t("Code {{code}}", { code: code.code })}>
            {code.code}
          </p>
          <p className="fine">{t("Valid until {{when}}.", { when: when(code.until) })}</p>
        </div>
      ) : (
        <form
          className="form"
          onSubmit={async (e) => {
            e.preventDefault();
            setErr(undefined);
            setBusy(true);
            try {
              setCode(await op<{ code: string; until: number }>("self.code", { device: device.trim() }));
              pollFast();
            } catch (x) {
              setErr(errText(x));
            } finally {
              setBusy(false);
            }
          }}
        >
          <label>
            {t("Device id (beside its QR code)")}
            <input value={device} onChange={(e) => setDevice(e.target.value)} placeholder="seca-mbca-01" spellCheck={false} required />
          </label>
          <Problem text={err} />
          <div className="actions">
            <button type="submit" className="primary" disabled={busy}>
              {busy ? t("Contacting the service…") : t("Get a code")}
            </button>
          </div>
        </form>
      )}
      <p className="fine">
        {t("Staff can also scan your identity code instead.")}{" "}
        <button type="button" className="link" onClick={() => go("account")}>
          {t("Show my identity code")}
        </button>
      </p>
    </section>
  );
}

/** Installs the OCR models (desktop: downloaded and checked by the core; elsewhere: the command to run). */
function OcrInstall({ done }: { done: () => void }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();
  if (!isDesktop)
    return (
      <div className="group notice">
        <p>{t("Install it once from a terminal:")}</p>
        <pre>health component install ocr</pre>
      </div>
    );
  return (
    <div className="group notice" aria-busy={busy}>
      <h3>{t("Read lab reports on this computer")}</h3>
      <p className="fine">
        {t("Health downloads the text-recognition models once (about 21 MB, plus 135 MB that suggests which measure each result is). Every file is checked before use, and your reports never leave this computer.")}
      </p>
      <Problem text={err} />
      <div className="actions">
        <button
          type="button"
          className="primary"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setErr(undefined);
            try {
              await installOcr(true);
              done();
            } catch (e) {
              setErr(errText(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          {busy ? t("Downloading…") : t("Install")}
        </button>
      </div>
    </div>
  );
}

const IMPORT_LEDE: Record<string, string> = {
  apple: tk("On your iPhone, open Health, tap your picture, then Export All Health Data; choose the .zip here."),
  report: tk("A photo (JPEG, PNG) or PDF of a lab report. Nothing counts until you check it under Waiting for you."),
  restore: tk("A Health backup (.zuhealth) from this app or the web version. Restoring merges; nothing is overwritten."),
  file: tk("A photo or PDF of a lab report, an Apple Health export, a Health backup (.zuhealth) or WHOOP data. Importing the same file again updates it; nothing is doubled."),
};

function Import({ file, onFile, go, kind: hint = "file", title }: { file?: string; onFile: (p?: string) => void; go: (p: Page) => void; kind?: string; title?: string }) {
  const [password, setPassword] = useState("");
  const [source, setSource] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();
  const [done, setDone] = useState<ReactNode>();
  const [needsOcr, setNeedsOcr] = useState(false);
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
      if (kind === "report") {
        // recognised in a separate process by the core, then kept as a batch you check before anything counts
        const r = await op<{ import_id: number; items: number; matched: number }>("import.ocr", { path: file, tz_min: -new Date().getTimezoneOffset() });
        setDone(
          <>
            <p>{t("Read {{count}} results. Nothing is added to your readings until you check them.", { count: r.items })}</p>
            <div className="actions">
              <button type="button" className="primary" onClick={() => go("review")}>
                {t("Check them now")}
              </button>
            </div>
          </>,
        );
        return;
      }
      if (kind === "backup") {
        const r = await op<BackupResult>("backup.import", { path: file, password });
        const notes: string[] = [];
        if (r.refused_imports + r.refused_corrections) notes.push(t("{{count}} entries were refused because their dates lie in the future, which means a damaged or altered file.", { count: r.refused_imports + r.refused_corrections }));
        if (r.skipped_metrics.length) notes.push(t("{{count}} measures were skipped because their unit differs on this computer: {{list}}.", { count: r.skipped_metrics.length, list: r.skipped_metrics.join(", ") }));
        if (r.missing_blobs.length) notes.push(t("{{count}} original files (such as lab report photos) are not in this backup; the readings from them are.", { count: r.missing_blobs.length }));
        setDone(
          <>
            <p>
              {r.erased_imports
                ? t("Restored: {{added}} imports added, {{erased}} removed because they were deleted on the other device. Nothing here was overwritten.", { added: r.added_imports, erased: r.erased_imports })
                : t("Restored: {{added}} imports added. Nothing here was overwritten.", { added: r.added_imports })}
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
              {t("Imported {{n}} readings from {{count}} devices.", { n: r.written.toLocaleString(), count: r.sources })}
              {r.added < r.written ? ` ${t("{{n}} were already here and were updated.", { n: (r.written - r.added).toLocaleString() })}` : ""}
            </p>
            {broken > 0 && <p className="fine">{t("{{n}} records had a bad date or value and were left out.", { n: broken.toLocaleString() })}</p>}
          </>,
        );
      }
      setPassword("");
    } catch (e) {
      if (e instanceof OpError && (e.code === "no_ocr_component" || e.code === "ocr_component")) setNeedsOcr(true);
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="record">
      <h2>{title ?? t("Import a file")}</h2>
      <p className="lede">{t(IMPORT_LEDE[hint] ?? IMPORT_LEDE.file!)}</p>
      <FilePick path={file} onPath={onFile} />
      {file && !kind && (
        <Problem text={t("Health can't read this kind of file. Choose a lab report photo or PDF, a .zip from Apple Health, a .zuhealth backup, or a WHOOP .json file.")} />
      )}
      {needsOcr && <OcrInstall done={() => (setNeedsOcr(false), setErr(undefined))} />}
      {file && kind && (
        <form
          className="form"
          aria-busy={busy}
          onSubmit={(e) => {
            e.preventDefault();
            run();
          }}
        >
          <p className="fine">{t(KIND_LABEL[kind])}</p>
          {kind === "backup" && <PasswordField label={t("Backup password")} value={password} onChange={setPassword} auto="current-password" />}
          {kind === "rows" && (
            <label>
              {t("Where these readings come from")}
              <input value={source} onChange={(e) => setSource(e.target.value)} required />
            </label>
          )}
          <Problem text={err} />
          <button type="submit" className="primary" disabled={busy}>
            {busy ? (kind === "backup" ? t("Restoring…") : t("Importing…")) : kind === "backup" ? t("Restore backup") : t("Import")}
          </button>
          {busy && (
            <p className="fine">
              {kind === "report"
                ? t("Reading the report: a few seconds a page.")
                : t("Large exports can take a few minutes. Health answers again when the import is done; keep this page open to see the result.")}
            </p>
          )}
        </form>
      )}
      {done && (
        <div className="group notice" role="status">
          {done}
          <div className="actions">
            <button type="button" className="primary" onClick={() => go("readings")}>
              {t("See my readings")}
            </button>
            <button type="button" onClick={() => onFile(undefined)}>
              {t("Import another file")}
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

function Trend({ p, days, latest }: { p: Point; days?: Day[]; latest: string }) {
  const pts = days ? toPts(p.agg, days) : [];
  return pts.length >= 2 ? <Spark pts={pts} label={nameOf(p.metric, p.name)} latest={latest} /> : null;
}

/** What to show for a reading. Summed measures (steps, energy) show their latest day's total, not the last slice. */
function display(p: Point, days?: Day[]): { value: string; unit: string; note?: string } {
  if (p.agg === "cumulative" && days?.length) {
    const last = days[days.length - 1]!;
    const today = last.day === day(Date.now());
    const note = today ? t("today") : ago(dayMs(last.day));
    const asTime = asDuration(last.sum ?? 0, p.unit, true);
    return asTime ? { value: asTime, unit: "", note } : { value: num(last.sum ?? 0), unit: unitLabel(p.unit), note };
  }
  return shown(p);
}

/** Generic device names ("OCR", "photo") read better as their kind. */
function sourceLabel(src?: Source): string {
  if (!src) return t("Source");
  if (src.connector === "manual") return t("Entered by you");
  const generic = ["ocr", "photo", src.connector].includes(src.name.toLowerCase());
  return generic ? sourceKind(src.connector) : src.name;
}

const STALE_DAYS = 60;

/** Out-of-range mark from a lab sheet (H/L printed next to the value). */
function Flag({ p }: { p: Point }) {
  const f = (p.detail?.flag ?? (p.detail?.printed as { flag?: string } | undefined)?.flag) as string | undefined;
  if (p.corrected || (f !== "H" && f !== "L")) return null;
  return (
    <span className="flag" title={f === "H" ? t("Above the reference range") : t("Below the reference range")}>
      {f === "H" ? t("↑ high") : t("↓ low")}
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
  // no trend to draw (lab values, old or sparse ones): the date of the value fills the space instead
  const trend = !old && !!days && toPts(p.agg, days).length >= 2;
  const note = s.note ?? (lab || old || !trend ? ago(p.start_ms) : undefined);
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
        {!old && days && <Trend p={p} days={days} latest={s.unit ? `${s.value} ${s.unit}` : s.value} />}
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
          {t(g.label)}
        </h3>
        {!all && rows.length > fold && (
          <button type="button" className="link" aria-expanded={more} onClick={() => setMore(!more)}>
            {more ? t("Show fewer") : t("Show all {{n}}", { n: rows.length })}
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
          {new Set(from).size > 3
            ? t("From {{list}} and {{n}} more", { list: [...new Set(from)].slice(0, 3).join(t(", ")), n: new Set(from).size - 3 })
            : t("From {{list}}", { list: [...new Set(from)].slice(0, 3).join(t(", ")) })}
        </p>
      )}
    </section>
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
  onSection,
}: {
  latest?: Point[];
  err?: string;
  open: (code: string) => void;
  go: (p: Page) => void;
  reviews: number;
  find: boolean;
  found: () => void;
  /** The category now in view (null: the top), for the sidebar */
  onSection: (key: string | null) => void;
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
  // which category is under the page's top edge: the sidebar marks it
  useEffect(() => {
    if (!latest?.length) return;
    const seen = new Map<string, boolean>();
    const io = new IntersectionObserver(
      (entries) => {
        for (const x of entries) seen.set(x.target.id.slice(2), x.isIntersecting);
        const first = GROUPS.find((g) => seen.get(g.key));
        onSection(window.scrollY < 40 ? null : (first?.key ?? null));
      },
      { rootMargin: "-80px 0px -60% 0px" },
    );
    for (const el of document.querySelectorAll("section.section[id^='g-']")) io.observe(el);
    const top = () => window.scrollY < 40 && onSection(null);
    window.addEventListener("scroll", top, { passive: true });
    return () => {
      io.disconnect();
      window.removeEventListener("scroll", top);
      onSection(null);
    };
  }, [latest, onSection]);
  if (err) return <Problem text={err} />;
  if (!latest) return <p className="empty" aria-busy="true">{t("Loading your readings…")}</p>;
  if (!latest.length)
    return (
      <section className="page">
        <h2>{t("Readings")}</h2>
        <p className="lede">
          {t("Nothing here yet. Restore a Health backup (.zuhealth), import an Apple Health or WHOOP export, or add a reading by hand. You can also drop a file anywhere on this window.")}
        </p>
        <div className="actions">
          <button type="button" className="primary" onClick={() => go("data")}>
            {t("Data sources")}
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
          <h2>{t("Readings")}</h2>
          <p className="fine">{t("Updated {{when}}", { when: when(newest) })}</p>
        </div>
        <div className="search-field">
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
            <circle cx="7" cy="7" r="4.5" />
            <path d="M10.5 10.5L14 14" />
          </svg>
          <input
            ref={search}
            type="search"
            placeholder={`${t("Find a measure")} (${MOD}F)`}
            aria-label={t("Find a measure")}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setQ("")}
          />
        </div>
      </div>
      {reviews > 0 && (
        <p className="notice">
          {t("{{count}} imports are waiting for you to check.", { count: reviews })}{" "}
          <button type="button" className="link" onClick={() => go("review")}>
            {t("Review now")}
          </button>
        </p>
      )}
      {!groups.length && <p className="empty">{t("No measure matches “{{q}}”.", { q })}</p>}
      {groups.map(({ g, rows }) => (
        <Section key={g.key} g={g} rows={rows} sources={sources ?? []} open={open} all={!!needle} />
      ))}
    </section>
  );
}

const RANGES = [
  { label: tk("30 days"), days: 30 },
  { label: tk("90 days"), days: 90 },
  { label: tk("1 year"), days: 365 },
  { label: tk("All"), days: 0 },
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
                {nameOf(m.code, m.name)} <span className="fine">{m.code}</span>
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
  // the newest 10 entries show; "Show all" asks for the rest
  const [allEntries, setAllEntries] = useState(false);
  const [points] = useOp(
    () => (code ? op<Point[]>("obs.query", { metric: code, limit: allEntries ? 100_000 : 30, newest_first: true }) : Promise.resolve([])),
    [code, allEntries],
  );
  const [sources] = useOp(() => op<Source[]>("source.list"), []);
  useEffect(() => {
    setRange(null);
    setAllEntries(false);
  }, [code]);
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
        <h2>{t("History")}</h2>
        <MetricPicker placeholder={t("Search measures, e.g. weight")} onPick={(m) => pick(m.code)} />
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
      <div className="segmented" role="group" aria-label={t("Range")}>
        {RANGES.map((r) => (
          <button key={r.label} type="button" aria-pressed={span === r.days} onClick={() => setRange(r.days)}>
            {t(r.label)}
          </button>
        ))}
      </div>
      {vals.length > 0 && (
        <dl className="stats">
          <div>
            <dt>{t("Average")}</dt>
            <dd>
              {fmt(vals.reduce((a, b) => a + b, 0) / vals.length)} <span className="unit">{shownUnit}</span>
            </dd>
          </div>
          <div>
            <dt>{t("Lowest")}</dt>
            <dd>
              {fmt(Math.min(...lows))} <span className="unit">{shownUnit}</span>
            </dd>
          </div>
          <div>
            <dt>{t("Highest")}</dt>
            <dd>
              {fmt(Math.max(...highs))} <span className="unit">{shownUnit}</span>
            </dd>
          </div>
          <div>
            <dt>{t("Days with data")}</dt>
            <dd>{vals.length}</dd>
          </div>
          {refText && (
            <div>
              <dt>{t("Reference range")}</dt>
              <dd>{refText}</dd>
            </div>
          )}
        </dl>
      )}
      <DailyChart pts={pts} unit={shownUnit} fmt={fmt} />
      <h3>{t("Latest entries")}</h3>
      <table className="rows">
        <tbody>
          {(allEntries ? recent : recent.slice(0, 10)).map((p) => {
            const s = shown(p);
            return (
              <tr key={`${p.start_ms}-${p.source_id}`}>
                <td>{p.detail?.precision === "day" ? dateIn(p.start_ms, p.tz_min) : when(p.start_ms, p.tz_min)}</td>
                <td className="fine">{sourceName(p.source_id)}</td>
                <td className="n value">
                  {s.value} <span className="unit">{s.unit}</span>
                  <Flag p={p} />
                  {p.corrected && <span className="fine"> {t("corrected")}</span>}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {!allEntries && recent.length > 10 && (
        <button type="button" className="link" onClick={() => setAllEntries(true)}>
          {t("Show all entries")}
        </button>
      )}
    </section>
  );
}

/** Common measures, one tap away (as the web version's record page). */
const QUICK: [string, string][] = [
  ["loinc:29463-7", tk("Weight")],
  ["loinc:8480-6", tk("Blood pressure, systolic")],
  ["loinc:8462-4", tk("Blood pressure, diastolic")],
  ["loinc:8867-4", tk("Heart rate")],
  ["loinc:2339-0", tk("Blood glucose")],
  ["loinc:8310-5", tk("Body temperature")],
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
    if (!trimmed) return setErr(t("Type a value."));
    const v = text ? trimmed : parseNumber(trimmed);
    if (!text && !Number.isFinite(v)) return setErr(t("Type a number, such as 72 or 36.6."));
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
    <section className="record">
      <h2>{t("Enter by hand")}</h2>
      {!m ? (
        <>
          <MetricPicker placeholder={t("What did you measure?")} onPick={pick} />
          <div className="chips" role="group" aria-label={t("Common measures")}>
            {QUICK.map(([code, label]) => (
              <button key={code} type="button" className="chip-button" onClick={() => pick({ code, name: label, unit: "", agg: "instant" })}>
                {t(label)}
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
            <strong>{nameOf(m.code, m.name)}</strong>{" "}
            <button type="button" className="link" onClick={() => setM(undefined)}>
              {t("Change")}
            </button>
          </p>
          <label>
            {t("Value")}
            <input inputMode={m.agg === "none" ? "text" : "decimal"} value={value} onChange={(e) => setValue(e.target.value)} required />
          </label>
          {m.agg !== "none" && value.includes(",") && Number.isFinite(parseNumber(value)) && (
            <p className="fine" role="status">
              {t("Read as {{n}}", { n: parseNumber(value).toLocaleString(undefined, { maximumFractionDigits: 6 }) })}
            </p>
          )}
          {m.units.length > 1 && (
            <label>
              {t("Unit")}
              <select value={unit} onChange={(e) => setUnit(e.target.value)} required>
                <option value="">{t("Choose the unit on your device")}</option>
                {m.units.map((u) => (
                  <option key={u} value={u}>
                    {unitLabel(u) || u}
                  </option>
                ))}
              </select>
            </label>
          )}
          <label>
            {t("When")}
            <input type="datetime-local" value={at} onChange={(e) => setAt(e.target.value)} required />
          </label>
          <Problem text={err} />
          <button type="submit" className="primary" disabled={busy}>
            {t("Save reading")}
          </button>
        </form>
      )}
    </section>
  );
}


function Sources(_: { go: (p: Page) => void }) {
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
    }, t("Order saved."));
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
    <section className="record">
      <h2>{t("Manage data")}</h2>
      <p className="lede">{t("Where your readings came from. Open a kind to see each import, or delete one: its readings go with it.")}</p>
      {msg && (
        <p className="notice" role="status">
          {msg}
        </p>
      )}
      <Problem text={err} />
      {batches && !batches.length && <p className="empty">{t("No imports yet.")}</p>}
      <div className="list-card">
        {ordered.map(([kind, list]) => {
          const devices = new Set(list.map((b) => b.source_id));
          const last = Math.max(...list.map((b) => b.at ?? 0));
          return (
            <details key={kind} className="kind">
              <summary>
                <span className="name">{sourceKind(kind)}</span>
                <span className="fine">
                  {t("{{count}} imports", { count: list.length })}
                  {devices.size > 1 ? `${t(", ")}${t("{{count}} devices", { count: devices.size })}` : ""} · {last ? shortDate(last) : ""}
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
                        {b.status === "review" ? <span className="signal">{t("waiting for you")}</span> : t("{{count}} measures", { count: b.metrics })}
                      </span>
                    </span>
                    {confirm === b.import_id ? (
                      <span className="actions">
                        <button type="button" className="danger" onClick={() => run(() => op("import.erase", { id: b.import_id }), t("Deleted, with its readings."))}>
                          {t("Delete import and its readings")}
                        </button>
                        <button type="button" onClick={() => setConfirm(undefined)}>
                          {t("Keep")}
                        </button>
                      </span>
                    ) : (
                      <button type="button" className="link" onClick={() => setConfirm(b.import_id)}>
                        {t("Delete…")}
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
          <h3>{t("When two sources report the same day")}</h3>
        </summary>
        <p className="fine">{t("The one higher in this list is used for daily summaries. Nothing is added up.")}</p>
        <ol className="order">
          {(sources ?? []).map((s, i) => (
            <li key={s.id}>
              <span>
                {s.connector === "manual" ? t("Entered by you") : s.name} <span className="fine">{sourceKind(s.connector)}</span>
              </span>
              <button type="button" className="link" disabled={i === 0 || ordering} aria-label={t("Prefer {{name}}", { name: s.name })} onClick={() => moveUp(i)}>
                {t("Move up")}
              </button>
            </li>
          ))}
        </ol>
      </details>
    </section>
  );
}

type OcrPick = { code?: string; new?: boolean; skip?: boolean; unit?: string };

/** One lab report waiting for review: each printed result → a measure (suggested, searched, or new) and its
 *  unit, or skipped; the test date for all of them. The core applies the rules (units, duplicates). */
function OcrReview({ b, done, discard }: { b: BatchDetail; done: (text: string) => void; discard: () => void }) {
  const c = b.candidates as { at: number; dated?: boolean; items: OcrItem[] };
  // no date on the report: the person sets it (the import time isn't the test date)
  const [date, setDate] = useState(c.dated === false ? "" : day(c.at));
  const [picks, setPicks] = useState<OcrPick[]>(() => c.items.map((it) => (it.code ? { code: it.code } : {})));
  const [other, setOther] = useState<number>();
  const [info, setInfo] = useState<Record<string, MetricInfo>>({});
  /** "<code> <printed unit>" → the measure's unit the printed one maps to ("" when none fits), from the core */
  const [picked, setPicked] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();
  // names and units of every measure on offer
  useEffect(() => {
    const codes = new Set<string>();
    c.items.forEach((it, i) => {
      if (it.code) codes.add(it.code);
      for (const s of it.suggest ?? []) codes.add(s.code);
      const p = picks[i]?.code;
      if (p) codes.add(p);
    });
    const missing = [...codes].filter((x) => !info[x]);
    if (missing.length)
      Promise.all(missing.map((code) => op<MetricInfo>("metric.show", { code }).then((m) => [code, m] as const, () => null))).then((got) =>
        setInfo((old) => ({ ...old, ...Object.fromEntries(got.filter((g): g is readonly [string, MetricInfo] => !!g)) })),
      );
    // which unit each chosen measure would take the printed one as
    const want = c.items.flatMap((it, i) => {
      const code = picks[i]?.code;
      return code && picked[`${code} ${it.unit}`] === undefined ? [[code, it.unit] as const] : [];
    });
    if (want.length)
      Promise.all(
        want.map(([code, unit]) =>
          op<MetricInfo & { picked_unit?: string }>("metric.show", { code, printed_unit: unit }).then(
            (m): readonly [string, string] => [`${code} ${unit}`, m.picked_unit ?? ""],
            () => null,
          ),
        ),
      ).then((got) => setPicked((old) => ({ ...old, ...Object.fromEntries(got.filter((g): g is readonly [string, string] => !!g)) })));
  }, [picks, c.items, info, picked]);
  const set = (i: number, p: OcrPick) => setPicks((old) => old.map((x, k) => (k === i ? p : x)));
  /** a numeric measure whose units don't include the printed one: the person must choose */
  const needsUnit = (i: number) => {
    const p = picks[i];
    const m = p?.code ? info[p.code] : undefined;
    if (!p?.code || !m || m.agg === "none" || m.agg === "interval" || !m.units.some((u) => u)) return false;
    return !p.unit && picked[`${p.code} ${c.items[i]!.unit}`] === "";
  };
  const count = picks.filter((p, i) => !p.skip && (p.code || p.new) && !needsUnit(i)).length;
  const save = async () => {
    setBusy(true);
    setErr(undefined);
    try {
      const choices = picks.map((p) => (p.skip ? { skip: true } : p.new ? { new: true } : p.code ? { code: p.code, ...(p.unit ? { unit: p.unit } : {}) } : {}));
      const r = await op<{ saved: number; left: number; created: number }>("review.confirm_ocr", {
        id: b.import_id,
        choices,
        date,
        tz_min: -new Date(`${date}T12:00`).getTimezoneOffset(),
      });
      done(
        (r.created
          ? t("Saved {{count}} results, with {{created}} new measures.", { count: r.saved, created: r.created })
          : t("Saved {{count}} results.", { count: r.saved })) +
          (r.left ? ` ${t("{{n}} left out: no measure, or a unit that doesn't fit it.", { n: r.left })}` : ""),
      );
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const label = (code: string) => (info[code] ? nameOf(code, info[code]!.name) : code);
  return (
    <div className="group ocr">
      <div className="ocr-head">
        <h3>{t("Lab report")}</h3>
        <label className="inline">
          {t("Test date")}
          <input type="date" value={date} onChange={(e) => setDate(e.target.value)} required />
        </label>
      </div>
      {c.dated === false && <p className="notice">{t("The date wasn't found on the report. Set the day the sample was taken.")}</p>}
      <table className="rows ocr-items">
        <thead>
          <tr>
            <th>{t("On the report")}</th>
            <th>{t("Result")}</th>
            <th>{t("Measure")}</th>
            <th>{t("Unit")}</th>
          </tr>
        </thead>
        <tbody>
          {c.items.map((it, i) => {
            const p = picks[i] ?? {};
            const m = p.code ? info[p.code] : undefined;
            const numeric = m && m.agg !== "none" && m.agg !== "interval";
            const value = p.skip ? "skip" : p.new ? "new" : (p.code ?? "");
            const offered = [...new Set([...(it.code ? [it.code] : []), ...(it.suggest ?? []).map((s) => s.code), ...(p.code ? [p.code] : [])])];
            return (
              <tr key={`${i}-${it.name}`} className={p.skip ? "stale" : ""}>
                <td>
                  {it.name}
                  {it.panel && <span className="fine"> · {it.panel}</span>}
                  {it.ref && <div className="fine">{t("Reference {{ref}}", { ref: it.ref })}</div>}
                </td>
                <td className="value">
                  {it.cmp ?? ""}
                  {String(it.v)} <span className="unit">{it.unit}</span>
                  {it.flag && (
                    <span className="flag" title={it.flag === "H" ? t("Marked high") : t("Marked low")}>
                      {it.flag === "H" ? "↑" : "↓"}
                    </span>
                  )}
                </td>
                <td>
                  <select
                    aria-label={t("Measure for {{name}}", { name: it.name })}
                    value={value}
                    onChange={(e) => {
                      const v = e.target.value;
                      setOther(undefined);
                      if (v === "skip") set(i, { skip: true });
                      else if (v === "new") set(i, { new: true });
                      else if (v === "other") setOther(i);
                      else set(i, v ? { code: v } : {});
                    }}
                  >
                    <option value="">{t("Choose…")}</option>
                    {offered.map((code) => (
                      <option key={code} value={code}>
                        {label(code)}
                      </option>
                    ))}
                    <option value="other">{t("Another measure…")}</option>
                    <option value="new">{t("New measure “{{name}}”", { name: it.name })}</option>
                    <option value="skip">{t("Skip")}</option>
                  </select>
                  {other === i && (
                    <MetricPicker
                      placeholder={t("Search measures")}
                      onPick={(x) => {
                        set(i, { code: x.code });
                        setOther(undefined);
                      }}
                    />
                  )}
                </td>
                <td>
                  {numeric && m.units.some((u) => u) && (m.units.length > 1 || needsUnit(i)) && (
                    <select aria-label={t("Unit for {{name}}", { name: it.name })} value={p.unit ?? ""} onChange={(e) => set(i, { ...p, unit: e.target.value || undefined })}>
                      <option value="">{picked[`${p.code} ${it.unit}`] ? t("As printed ({{unit}})", { unit: it.unit }) : t("Choose the unit…")}</option>
                      {m.units.map((u) => (
                        <option key={u} value={u}>
                          {unitLabel(u) || u}
                        </option>
                      ))}
                    </select>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <Problem text={err} />
      <div className="actions">
        <button type="button" className="primary" disabled={busy || !count || !date} onClick={save}>
          {t("Save {{count}} results", { count })}
        </button>
        <button type="button" onClick={discard}>
          {t("Discard")}
        </button>
      </div>
    </div>
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
      <h2>{t("Waiting for you")}</h2>
      <p className="lede">{t("Nothing here is visible in your readings until you confirm it.")}</p>
      {msg &&
        (msg.ok ? (
          <p className="fine" role="status">
            {msg.text}
          </p>
        ) : (
          <Problem text={msg.text} />
        ))}
      {items && !items.length && <p className="empty">{t("Nothing is waiting.")}</p>}
      {(items ?? []).map((b) => {
        if (b.candidates?.kind === "ocr")
          return (
            <OcrReview
              key={b.import_id}
              b={b}
              done={(text) => (setMsg({ ok: true, text }), reload())}
              discard={() => act("review.discard", b.import_id, t("Discarded."))}
            />
          );
        if (b.candidates?.kind === "result")
          return (
            <ResultReview
              key={b.import_id}
              b={b}
              done={(text) => (setMsg({ ok: true, text }), reload())}
              discard={() => act("review.discard", b.import_id, t("Discarded."))}
            />
          );
        const proposal = b.candidates?.kind === "proposal";
        const rows = (b.candidates?.rows ?? []) as { metric: string; start_ms: number; tz_min?: number; value: unknown; unit?: string }[];
        const by = proposal ? agent(b.candidates?.by) : undefined;
        return (
          <div key={b.import_id} className="group">
            <h3>{proposal ? (by ? t("Proposed by {{name}}", { name: by }) : t("Proposed by an agent")) : t("From a lab report photo")}</h3>
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
              <p className="fine">{t("Matching lab items to measures arrives with photo import.")}</p>
            )}
            <div className="actions">
              {proposal && (
                <button type="button" className="primary" onClick={() => act("review.confirm", b.import_id, t("Added to your readings."))}>
                  {t("Add to my readings")}
                </button>
              )}
              <button type="button" onClick={() => act("review.discard", b.import_id, t("Discarded."))}>
                {t("Discard")}
              </button>
            </div>
          </div>
        );
      })}
    </section>
  );
}

const windowText = (w: Grant["window"]) =>
  w === "all" ? t("all time") : "last_days" in w ? t("last {{n}} days", { n: w.last_days }) : t("a date range");
const writeText = (w: "none" | "propose" | "derived") =>
  ({ none: t("read only"), propose: t("may propose readings"), derived: t("may propose and write computed results") })[w];

const EXPIRY: [number, string][] = [
  [30, tk("30 days")],
  [90, tk("90 days")],
  [365, tk("1 year")],
];

/** After a grant is made: how to plug it into the chosen app, one click where the shell can do it. */
function ConnectSteps({ client, token, bin, found, done }: { client: Client; token: string; bin: string; found: boolean; done: () => void }) {
  const [result, setResult] = useState<{ ok: boolean; text: string }>();
  const [busy, setBusy] = useState(false);
  const oneClick = client.oneClick && isDesktop;
  return (
    <div className="group notice connect" role="status">
      <h3>{t("Connect {{name}}", { name: client.name })}</h3>
      {oneClick && (
        <>
          <p>
            {found ? t("{{name}} is on this computer.", { name: client.name }) : t("{{name}}'s settings folder wasn't found; it is created if you go ahead.", { name: client.name })}{" "}
            {t("Health can add itself to its settings for you. Other servers there are kept, and the original file is saved next to it (.health-bak).")}
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
                  setResult({ ok: true, text: t("Added to {{file}}. Restart {{name}} to pick it up.", { file: f, name: client.name }) });
                } catch (e) {
                  setResult({ ok: false, text: String(e) });
                }
                setBusy(false);
              }}
            >
              {t("Add to {{name}}", { name: client.name })}
            </button>
          </div>
          {result && <p className={result.ok ? "fine" : "problem"}>{result.text}</p>}
          <p className="fine">{t("Or do it by hand:")}</p>
        </>
      )}
      <ol className="steps">
        {client.steps(bin, token).map((st) => (
          <li key={st.text}>
            {st.text}
            {st.code && (
              <div className="snippet">
                <pre>{st.code}</pre>
                <Copy text={st.code} label={t("Copy")} />
              </div>
            )}
          </li>
        ))}
      </ol>
      <p className="fine">
        {t("The token is shown only this once. The app works with your data even when Health is closed (with a passphrase lock, keep Health open and unlocked). If you move the Health app, or the access expires, connect again. Apps that run in a web browser (such as ChatGPT or claude.ai) can't reach Health: it only answers on this computer.")}
      </p>
      <button type="button" className="link" onClick={done}>
        {t("Done")}
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
        {t("Revoke")}
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
        {t("Revoke {{name}}", { name })}
      </button>
      <button type="button" onClick={() => setAsk(false)}>
        {t("Keep")}
      </button>
      {err && <Problem text={err} />}
    </span>
  );
}

function LastUsed({ id }: { id: number }) {
  const [last, , reload] = useOp(() => op<Audit[]>("audit.list", { grant_id: id, limit: 1 }), [id]);
  usePoll(reload);
  if (!last) return null;
  return <span className="fine">{last[0] ? t("last used {{when}}", { when: ago(last[0].at) }) : t("not used yet")}</span>;
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
  const [profile, setProfile] = useState(false);
  const [connecting, setConnecting] = useState(false);
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
        profile,
        window: days ? { last_days: days } : "all",
        write,
        expires_ms: Date.now() + expiry * DAY,
      });
      setMade({ client, token: r.token });
      setClient(undefined);
      setConnecting(false);
      reload();
    } catch (e) {
      setMsg(errText(e));
    }
  };
  const toggle = (k: string) => setGroups((g) => (g.includes(k) ? g.filter((x) => x !== k) : [...g, k]));
  const ordered = [...CLIENTS].sort((a, b) => Number(found.includes(b.id)) - Number(found.includes(a.id)));
  return (
    <section className="page">
      <div className="page-head">
        <h2>{t("Agent access")}</h2>
        <div className="actions">
          <button type="button" className="primary" onClick={() => (setMade(undefined), setConnecting(true))}>
            {t("Connect an app…")}
          </button>
        </div>
      </div>
      <p className="lede">
        {t("Let an AI app (Claude, Codex, Cursor, Copilot, Gemini and others) read your data through MCP. It sees only what you allow here, can never delete anything or read your backups, and anything it writes waits for you or lands under its own computed measures.")}
      </p>
      {made && <ConnectSteps client={made.client} token={made.token} bin={bin} found={found.includes(made.client.id)} done={() => setMade(undefined)} />}
      {connecting && (
        <Sheet label={t("Connect an app")} close={() => (setConnecting(false), setClient(undefined))}>
        <div className="group">
          <h3>{t("Connect an app")}</h3>
          <div className="clients" role="group" aria-label={t("Agent apps")}>
            {ordered.map((c) => (
              <button key={c.id} type="button" className="client" aria-pressed={client?.id === c.id} onClick={() => pick(c)}>
                {c.name}
                {found.includes(c.id) && <span className="fine">{t("On this computer")}</span>}
              </button>
            ))}
          </div>
        </div>
          {client && (

        <form
          className="group form"
          onSubmit={(e) => {
            e.preventDefault();
            create();
          }}
        >
          <h3>{client.id === "other" ? t("What the app may see") : t("What {{name}} may see", { name: client.name })}</h3>
          <label>
            {t("Name shown in the access log")}
            <input placeholder={t("My agent")} value={name} onChange={(e) => setName(e.target.value)} required />
          </label>
          {[false, true].map((sensitive) => (
            <fieldset key={String(sensitive)}>
              <legend>{sensitive ? t("Sensitive: tick only what the agent needs") : t("What it may read")}</legend>
              {GROUPS.filter((g) => g.key !== "derived" && g.sensitive === sensitive).map((g) => (
                <label key={g.key} className="check">
                  <input type="checkbox" checked={groups.includes(g.key)} onChange={() => toggle(g.key)} />
                  {t(g.label)}
                </label>
              ))}
            </fieldset>
          ))}
          <label className="check">
            <input type="checkbox" checked={detail} onChange={(e) => setDetail(e.target.checked)} />
            {t("Every reading (otherwise daily summaries only)")}
          </label>
          <label className="check">
            <input type="checkbox" checked={reports} onChange={(e) => setReports(e.target.checked)} />
            {t("Full text of lab reports (shows every item on the sheet, whatever is ticked above)")}
          </label>
          <label className="check">
            <input type="checkbox" checked={profile} onChange={(e) => setProfile(e.target.checked)} />
            {t("Age and sex from your profile (not your birth month)")}
          </label>
          <label>
            {t("How far back")}
            <select value={days} onChange={(e) => setDays(Number(e.target.value))}>
              <option value={30}>{t("30 days")}</option>
              <option value={90}>{t("90 days")}</option>
              <option value={365}>{t("1 year")}</option>
              <option value={0}>{t("Everything")}</option>
            </select>
          </label>
          <label>
            {t("Writing")}
            <select value={write} onChange={(e) => setWrite(e.target.value as Grant["write"])}>
              <option value="none">{t("Read only")}</option>
              <option value="propose">{t("May propose readings for me to confirm")}</option>
              <option value="derived">{t("May also write computed results")}</option>
            </select>
          </label>
          <label>
            {t("Access ends after")}
            <select value={expiry} onChange={(e) => setExpiry(Number(e.target.value))}>
              {EXPIRY.map(([d, l]) => (
                <option key={d} value={d}>
                  {t(l)}
                </option>
              ))}
            </select>
          </label>
          <Problem text={msg} />
          <div className="actions">
            <button type="submit" className="primary" disabled={!groups.length}>
              {t("Create access")}
            </button>
            <button type="button" onClick={() => setClient(undefined)}>
              {t("Cancel")}
            </button>
          </div>
        </form>
          )}
        </Sheet>
      )}
      <div className="group">
        <h3>{t("Current access")}</h3>
        <Problem text={err} />
        {grants && !grants.length && <p className="empty">{t("No app has access yet.")}</p>}
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
                      {g.groups.map(groupLabel).join(t(", "))}; {g.detail ? t("every reading") : t("daily summaries")}
                      {t(", ")}
                      {windowText(g.window)}
                      {t(", ")}
                      {writeText(g.write)}
                    </td>
                    <td>
                      {live
                        ? t("until {{date}}", { date: new Date(g.expires_ms).toLocaleDateString(i18nLocale()) })
                        : g.revoked_ms
                          ? t("revoked")
                          : t("expired")}
                    </td>
                    <td className="n">
                      {live && <Revoke id={g.id} name={g.name} done={reload} />}
                    </td>
                  </tr>
                );
              })}
          </tbody>
        </table>
      </div>
      <AuditLog />
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
    <div className="group">
      <h3>{t("Access log")}</h3>
      <p className="fine">{t("Every call an agent made: what it asked for and how many rows it got back. Values are never logged.")}</p>
      <Problem text={err} />
      {log && !log.length && <p className="empty">{t("No agent has read anything yet.")}</p>}
      <table className="rows">
        <tbody>
          {(log ?? []).map((a, i) => (
            <tr key={`${a.at}-${i}`}>
              <td>{when(a.at)}</td>
              <th scope="row">{nameOf(a.grant_id)}</th>
              <td>{a.op}</td>
              <td>{a.metrics.join(", ")}</td>
              <td className="n">{a.denied ? <span className="signal">{t("refused")}</span> : t("{{count}} rows", { count: a.rows })}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Backup and export (as the web's 备份与导出): save a backup, restore one, or write a table for other tools. */
function Backup() {
  const [sheet, setSheet] = useState<"save" | "restore" | "csv">();
  const [file, setFile] = useState<string>();
  const titles = { save: t("Save a backup"), restore: t("Restore a backup"), csv: t("Export a table (CSV)") };
  return (
    <section className="page">
      <h2>{t("Backup")}</h2>
      <p className="lede">{t("A backup is one encrypted file with everything in Health. It also opens in the web version.")}</p>
      <ul className="rows-card">
        <Row icon={<Icon k="backup" />} label={titles.save} sub={t("Encrypted with a password you choose")} onClick={() => setSheet("save")} />
        <Row icon={<Icon k="data" />} label={titles.restore} sub={t("Merges; nothing is overwritten")} onClick={() => setSheet("restore")} />
        <Row icon={<GroupIcon k="other" />} label={titles.csv} sub={t("Unencrypted, for spreadsheets and other tools")} onClick={() => setSheet("csv")} />
      </ul>
      {sheet && (
        <Sheet label={titles[sheet]} close={() => (setSheet(undefined), setFile(undefined))}>
          {sheet === "save" && <BackupSave />}
          {sheet === "restore" && <Import file={file} onFile={setFile} go={() => setSheet(undefined)} kind="restore" title={titles.restore} />}
          {sheet === "csv" && <CsvExport />}
        </Sheet>
      )}
    </section>
  );
}

/** A table of readings for other tools (web 导出表格): chosen groups only; unencrypted, so it says so. */
function CsvExport() {
  const [groups, setGroups] = useState<string[]>(GROUPS.filter((g) => !g.sensitive).map((g) => g.key));
  const [dateOnly, setDateOnly] = useState(false);
  const [out, setOut] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();
  const [done, setDone] = useState<string>();
  const save = async () => {
    setErr(undefined);
    setDone(undefined);
    let path = out;
    if (isDesktop) {
      const p = await chooseSavePath(`health-${day(Date.now())}.csv`, "CSV", "csv");
      if (!p) return;
      path = p;
    }
    if (!path) return setErr(t("Choose where to save the table."));
    setBusy(true);
    try {
      const r = await op<{ path: string; rows: number }>("export.csv", { path, groups, date_only: dateOnly });
      setDone(t("Saved {{file}}: {{count}} rows. It isn't encrypted: keep it somewhere safe.", { file: fileName(r.path), count: r.rows }));
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="record">
      <h2>{t("Export a table (CSV)")}</h2>
      <p className="lede">{t("One row per reading, for spreadsheets and other tools. The file isn't encrypted.")}</p>
      <form
        className="form"
        aria-busy={busy}
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        {[false, true].map((sensitive) => (
          <fieldset key={String(sensitive)}>
            <legend>{sensitive ? t("Sensitive: tick only what you need") : t("What to include")}</legend>
            {GROUPS.filter((g) => g.sensitive === sensitive).map((g) => (
              <label key={g.key} className="check">
                <input
                  type="checkbox"
                  checked={groups.includes(g.key)}
                  onChange={() => setGroups((x) => (x.includes(g.key) ? x.filter((k) => k !== g.key) : [...x, g.key]))}
                />
                {t(g.label)}
              </label>
            ))}
          </fieldset>
        ))}
        <label className="check">
          <input type="checkbox" checked={dateOnly} onChange={(e) => setDateOnly(e.target.checked)} />
          {t("Dates only (no times or time zones)")}
        </label>
        {!isDesktop && (
          <label>
            {t("Save to")}
            <input value={out} onChange={(e) => setOut(e.target.value)} placeholder="/Users/you/Documents/health.csv" />
          </label>
        )}
        <Problem text={err} />
        <button type="submit" className="primary" disabled={busy || !groups.length}>
          {busy ? t("Saving…") : isDesktop ? t("Save table…") : t("Save table")}
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

function BackupSave() {
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
    if (pw.length < 8) return setErr(t("Use at least 8 characters."));
    if (pw !== pw2) return setErr(t("The two passwords differ."));
    let path = out;
    if (isDesktop) {
      const p = await chooseSavePath(`health-${stamp}.zuhealth`, t("Health backup"), "zuhealth");
      if (!p) return;
      path = p;
    }
    if (!path) return setErr(t("Choose where to save the backup."));
    setBusy(true);
    try {
      // the native save dialog already asked before replacing a file
      const r = await op<{ bytes: number; path: string }>("backup.export", { path, password: pw, replace: isDesktop });
      setDone(
        t("Saved {{file}} ({{mb}} MB). Keep the password safe: without it the backup cannot be opened.", { file: fileName(r.path), mb: (r.bytes / 1e6).toFixed(1) }),
      );
      setPw("");
      setPw2("");
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="record">
      <h2>{t("Save a backup")}</h2>
      <form
        className="group form"
        aria-busy={busy}
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <PasswordField label={t("Password")} value={pw} onChange={setPw} auto="new-password" />
        <PasswordField label={t("Repeat password")} value={pw2} onChange={setPw2} auto="new-password" />
        <p className="fine">{t("At least 8 characters. There is no way to recover a forgotten backup password.")}</p>
        {!isDesktop && (
          <label>
            {t("Save to")}
            <input value={out} onChange={(e) => setOut(e.target.value)} placeholder={`/Users/you/Documents/health-${stamp}.zuhealth`} />
          </label>
        )}
        <Problem text={err} />
        <button type="submit" className="primary" disabled={busy}>
          {busy ? t("Saving…") : isDesktop ? t("Save backup…") : t("Save backup")}
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

// ---- Health account (protocol 1, 3.1, 3.5): the identity code, linking, the inbox --------------------------

type AccountInfo = { id: string; registered: boolean; identity?: string; drops: number };
type LinkState =
  | { state: "none" | "expired" }
  | { state: "waiting"; channel: string; expires_ms: number }
  | { state: "received"; channel: string; check: string; account: string; replaces: boolean }
  | { state: "failed"; code: string };
type Polled = { status: string; saved: number; review: number; seen: number; dropped: number };

/** Fast inbox polling (every 30 s) for an hour after the identity code was shown (protocol 3.2). */
let fastUntil = 0;
export const pollFast = () => {
  fastUntil = Date.now() + 3_600_000;
};

/** Fetches results while the app is open: at once, every 5 minutes, every 30 s in the fast hour. Sends
 *  nothing while there is no registered account (the core answers `nokey` without a request). */
function useInbox() {
  useEffect(() => {
    let last = 0;
    let busy = false;
    const tick = async () => {
      const due = Date.now() - last >= (Date.now() < fastUntil ? 30_000 : 300_000);
      if (!due || busy) return;
      busy = true;
      try {
        await op<Polled>("inbox.poll");
        last = Date.now();
        // WHOOP, when connected: the core skips it if the last sync was under an hour ago
        await op("whoop.sync").catch(() => {});
      } catch {
        // the core starting or locked, or the service down: try again in a minute
        last = Date.now() - (Date.now() < fastUntil ? 0 : 240_000);
      } finally {
        busy = false;
      }
    };
    tick();
    const t = setInterval(tick, 10_000);
    return () => clearInterval(t);
  }, []);
}

function Qr({ text, label }: { text: string; label: string }) {
  const [svg, setSvg] = useState<string>();
  useEffect(() => {
    op<{ svg: string }>("account.qr", { text }).then((r) => setSvg(r.svg), () => setSvg(undefined));
  }, [text]);
  return svg ? <div className="qr" role="img" aria-label={label} dangerouslySetInnerHTML={{ __html: svg }} /> : <div className="qr" />;
}

function HealthAccount({ go }: { go: (p: Page) => void }) {
  const [acc, setAcc] = useState<AccountInfo | null>();
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [linking, setLinking] = useState(false);
  const [giving, setGiving] = useState(false);
  const [polled, setPolled] = useState<string>();
  const [waiting, setWaiting] = useState(false);
  const load = useCallback(() => {
    op<{ account: AccountInfo | null }>("account.show").then((r) => setAcc(r.account), (e) => setErr(errText(e)));
  }, []);
  useEffect(load, [load]);
  const show = async () => {
    setErr(undefined);
    setBusy(true);
    try {
      await op("account.register");
      pollFast();
      load();
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const check = async () => {
    setErr(undefined);
    setBusy(true);
    try {
      const r = await op<Polled>("inbox.poll");
      const parts = [
        r.saved && t("{{count}} new results saved", { count: r.saved }),
        r.review && t("{{count}} waiting for you to check", { count: r.review }),
        r.dropped && t("{{count}} discarded (failed a check)", { count: r.dropped }),
      ].filter(Boolean);
      setPolled(parts.length ? `${parts.join(t(", "))}${t(".")}` : t("No new results."));
      setWaiting(r.review > 0);
      load();
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  if (acc === undefined) return <section className="page narrow" aria-busy="true"><h2>{t("Health account")}</h2></section>;
  if (linking) return <LinkThis done={() => { setLinking(false); load(); }} />;
  if (giving) return <GiveAccount done={() => { setGiving(false); load(); }} />;
  return (
    <section className="page narrow">
      <h2>{t("Health account")}</h2>
      <p className="lede">
        {t("Labs and service desks send results to your health account, sealed so only your devices can open them. Your account key stays on your devices; Health sends no readings anywhere.")}
      </p>
      {acc?.identity ? (
        <div className="group identity">
          <Qr text={acc.identity} label={t("Your identity code")} />
          <div>
            <h3>{t("Your identity code")}</h3>
            <p className="fine">{t("Show it at a lab or service desk so they can send your results here.")}</p>
            <p className="fine mono">{t("Account {{id}}…", { id: acc.id.slice(0, 8) })}</p>
          </div>
        </div>
      ) : (
        <div className="group form">
          <p>{acc ? t("This computer has an account, but its identity code hasn't been shown yet.") : t("There is no health account on this computer yet.")}</p>
          <div className="actions">
            <button type="button" className="primary" disabled={busy} onClick={show}>
              {busy ? t("Contacting the service…") : acc ? t("Show my identity code") : t("Create my account")}
            </button>
            <button type="button" disabled={busy} onClick={() => setLinking(true)}>
              {t("Link an existing device…")}
            </button>
          </div>
        </div>
      )}
      <Problem text={err} />
      {acc && (
        <div className="group form">
          <h3>{t("Results")}</h3>
          <p className="fine">{t("Health checks for new results when it opens and every few minutes while it's open.")}</p>
          {acc.drops > 0 && <p className="fine">{t("{{count}} results were discarded because they failed a check.", { count: acc.drops })}</p>}
          <div className="actions">
            <button type="button" disabled={busy || !acc.registered} onClick={check}>
              {t("Check for results now")}
            </button>
            {waiting && (
              <button type="button" className="link" onClick={() => go("review")}>
                {t("Check them now")}
              </button>
            )}
          </div>
          {polled && <p role="status" className="fine">{polled}</p>}
        </div>
      )}
      {acc && (
        <div className="group form">
          <h3>{t("Devices")}</h3>
          <ul className="rows-card">
            <Row icon={<Icon k="account" />} label={t("Link an existing device")} sub={t("Use the account from your other device on this computer")} onClick={() => setLinking(true)} />
            <Row icon={<Icon k="account" />} label={t("Link a new device")} sub={t("Hand this computer's account to a new device")} onClick={() => setGiving(true)} />
          </ul>
          <LinkHistoryList />
        </div>
      )}
    </section>
  );
}

/** This computer joins an account on another device: the code to scan, then the check code (protocol 3.5). */
function LinkThis({ done }: { done: () => void }) {
  const [code, setCode] = useState<string>();
  // the link this screen started: adopt only that one
  const [channel, setChannel] = useState<string>();
  const [st, setSt] = useState<LinkState>({ state: "none" });
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  const start = useCallback(async () => {
    setErr(undefined);
    try {
      const j = await op<{ code: string; channel: string; expires_ms: number }>("link.join");
      setCode(j.code);
      setChannel(j.channel);
      setSt({ state: "waiting", channel: j.channel, expires_ms: j.expires_ms });
    } catch (e) {
      setErr(errText(e));
    }
  }, []);
  useEffect(() => {
    start();
    return () => {
      op("link.cancel").catch(() => {});
    };
  }, [start]);
  useEffect(() => {
    if (st.state !== "waiting") return;
    const t = setTimeout(async () => {
      try {
        setSt(await op<LinkState>("link.poll"));
      } catch (e) {
        setErr(errText(e));
      }
    }, 2000);
    return () => clearTimeout(t);
  }, [st]);
  const adopt = async () => {
    if (st.state !== "received") return;
    setBusy(true);
    try {
      await op("link.adopt", { channel, check: st.check });
      await op("account.register").catch(() => {});
      pollFast();
      done();
    } catch (e) {
      setErr(errText(e));
      setBusy(false);
    }
  };
  return (
    <section className="page narrow">
      <h2>{t("Link an existing device")}</h2>
      {st.state === "waiting" && code && (
        <div className="group identity">
          <Qr text={code} label={t("Link code")} />
          <div>
            <p>{t("On the device that has your account, open Health › Link a new device and scan this code.")}</p>
            <p className="fine">{t("From another computer, paste this code instead:")}</p>
            <p className="fine mono link-code">{code}</p>
            <p className="fine">{t("Waiting… The code works for 10 minutes.")}</p>
          </div>
        </div>
      )}
      {st.state === "received" && (
        <div className="group form">
          <p>{t("Check that your other device shows the same code:")}</p>
          <p className="check-code" aria-label={t("Check code {{code}}", { code: st.check })}>{st.check}</p>
          {st.replaces && (
            <p className="notice">{t("This computer's identity code will change to the other device's. Results not yet received here won't arrive.")}</p>
          )}
          <div className="actions">
            <button type="button" className="primary" disabled={busy} onClick={adopt}>
              {busy ? t("Linking…") : t("They match")}
            </button>
            <button type="button" disabled={busy} onClick={done}>
              {t("They don't match")}
            </button>
          </div>
        </div>
      )}
      {(st.state === "expired" || st.state === "failed") && (
        <div className="group form">
          <p>
            {st.state === "expired" ? t("The code expired.") : t("The link failed.")} {t("Nothing was changed on this computer.")}
          </p>
          <div className="actions">
            <button type="button" className="primary" onClick={start}>
              {t("Start again")}
            </button>
          </div>
        </div>
      )}
      <Problem text={err} />
      <div className="actions">
        <button type="button" className="link" disabled={busy} onClick={done}>
          {t("Cancel")}
        </button>
      </div>
    </section>
  );
}

/** This computer hands its account to a new device showing a link code (protocol 3.5 rule 2). */
function GiveAccount({ done }: { done: () => void }) {
  const [code, setCode] = useState("");
  const [sure, setSure] = useState(false);
  const [check, setCheck] = useState<string>();
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  const give = async () => {
    setErr(undefined);
    setBusy(true);
    try {
      const r = await op<{ check: string }>("link.give", { code: code.trim(), confirmed: true });
      setCheck(r.check);
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="page narrow">
      <h2>{t("Link a new device")}</h2>
      {check ? (
        <div className="group form">
          <p>{t("The new device must show the same code:")}</p>
          <p className="check-code" aria-label={t("Check code {{code}}", { code: check })}>{check}</p>
          <div className="actions">
            <button type="button" className="primary" onClick={done}>
              {t("Done")}
            </button>
          </div>
        </div>
      ) : (
        <form
          className="group form"
          onSubmit={(e) => {
            e.preventDefault();
            give();
          }}
        >
          <label>
            {t("Link code shown on the new device")}
            <input value={code} onChange={(e) => setCode(e.target.value)} placeholder="SBHL1:…" spellCheck={false} autoComplete="off" />
          </label>
          <label className="check">
            <input type="checkbox" checked={sure} onChange={(e) => setSure(e.target.checked)} />
            {t("This hands my health account to the device showing this code. It is my own device.")}
          </label>
          <Problem text={err} />
          <div className="actions">
            <button type="submit" className="primary" disabled={busy || !sure || !code.trim()}>
              {busy ? t("Sending…") : t("Hand over my account")}
            </button>
            <button type="button" onClick={done}>
              {t("Cancel")}
            </button>
          </div>
        </form>
      )}
    </section>
  );
}

type ResultLine = { code: string; v: unknown; u?: string; t?: number };
type LineState = { metric?: MetricInfo; unit: string; skip: boolean };
const numericAgg = (m: MetricInfo) => m.agg !== "none" && m.agg !== "interval";

/** A result sent to the account whose measure or unit Health couldn't settle (protocol 2.3 step 4): a measure
 *  and unit per line, or skip it (W review-form.ts). Known codes and fitting units come preselected. */
function ResultReview({ b, done, discard }: { b: BatchDetail; done: (text: string) => void; discard: () => void }) {
  const c = b.candidates as unknown as { src: string; at: number; tz: number; lines: ResultLine[] };
  const [lines, setLines] = useState<LineState[]>(() => c.lines.map(() => ({ unit: "", skip: false })));
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  const choose = useCallback(
    async (i: number, code: string) => {
      const l = c.lines[i]!;
      const m = await op<MetricInfo & { picked_unit?: string }>("metric.show", { code, ...(l.u ? { printed_unit: l.u } : {}) });
      setLines((ls) => ls.map((s, j) => (j === i ? { metric: m, unit: numericAgg(m) ? (m.picked_unit ?? "") : "", skip: false } : s)));
    },
    [c.lines],
  );
  // codes Health knows are preselected, once per batch: a reload of the list mustn't undo the person's choices
  const preselected = useRef<number>(undefined);
  useEffect(() => {
    if (preselected.current === b.import_id) return;
    preselected.current = b.import_id;
    c.lines.forEach((l, i) => {
      choose(i, l.code).catch(() => {});
    });
  }, [b.import_id, c.lines, choose]);
  const ready = lines.every((s) => s.skip || (s.metric && (!numericAgg(s.metric) || s.metric.units.length === 0 || s.unit))) && lines.some((s) => !s.skip);
  const save = async () => {
    setErr(undefined);
    setBusy(true);
    try {
      const choices = lines.map((s) => (s.skip ? { skip: true } : { code: s.metric?.code, ...(s.unit ? { unit: s.unit } : {}) }));
      const r = await op<{ written: number }>("review.confirm_result", { id: b.import_id, choices });
      done(t("Saved {{count}} readings.", { count: r.written }));
    } catch (e) {
      setErr(errText(e));
      setBusy(false);
    }
  };
  const [kind, device] = [c.src.split(":")[0], c.src.split(":").slice(1).join(":")];
  const from =
    kind === "lab" ? t("Lab device {{id}}", { id: device }) : kind === "staff" ? t("Service desk {{id}}", { id: device }) : t("Room {{id}}", { id: device });
  return (
    <div className="group ocr">
      <h3>{t("A result from {{from}}", { from })}</h3>
      <p className="fine">
        {when(c.at, c.tz)}
        {t(".")} {t("Choose what each line is, or skip it.")}
      </p>
      <table className="rows">
        <tbody>
          {c.lines.map((l, i) => {
            const s = lines[i]!;
            return (
              <tr key={`${l.code}-${i}`} className={s.skip ? "skipped" : undefined}>
                <td className="mono">{l.code}</td>
                <td className="n">
                  {String(l.v)} <span className="unit">{unitLabel(l.u ?? "")}</span>
                </td>
                <td>
                  {s.metric ? (
                    <span>
                      {nameOf(s.metric.code, s.metric.name)}{" "}
                      <button
                        type="button"
                        className="link"
                        onClick={() => setLines((ls) => ls.map((x, j) => (j === i ? { unit: "", skip: false } : x)))}
                      >
                        {t("Change")}
                      </button>
                    </span>
                  ) : (
                    <MetricPicker placeholder={t("Which measure is this?")} onPick={(m) => choose(i, m.code)} />
                  )}
                </td>
                <td>
                  {s.metric && numericAgg(s.metric) && (s.metric.units.length > 1 || (s.metric.units.length === 1 && !s.unit)) && (
                    <select
                      aria-label={t("Unit for {{name}}", { name: l.code })}
                      value={s.unit}
                      onChange={(e) => setLines((ls) => ls.map((x, j) => (j === i ? { ...x, unit: e.target.value } : x)))}
                    >
                      <option value="">{t("Unit…")}</option>
                      {s.metric.units.map((u) => (
                        <option key={u} value={u}>
                          {unitLabel(u)}
                        </option>
                      ))}
                    </select>
                  )}
                </td>
                <td>
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={s.skip}
                      onChange={(e) => setLines((ls) => ls.map((x, j) => (j === i ? { ...x, skip: e.target.checked } : x)))}
                    />
                    {t("Skip")}
                  </label>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <Problem text={err} />
      <div className="actions">
        <button type="button" className="primary" disabled={!ready || busy} onClick={save}>
          {busy ? t("Saving…") : t("Save to my readings")}
        </button>
        <button type="button" onClick={discard}>
          {t("Discard")}
        </button>
      </div>
    </div>
  );
}

type WhoopStatus = { state: "none" | "pending" | "reauth" } | { state: "connected"; last_sync: number | null };

/** WHOOP sync with the person's own WHOOP developer app (protocol 6, decision 67): sign-in in the browser,
 *  back to this computer, then hourly while Health is open. */
function WhoopConnect() {
  const [st, setSt] = useState<WhoopStatus>();
  const [id, setId] = useState("");
  const [secret, setSecret] = useState("");
  const [redirect, setRedirect] = useState("http://localhost:47630/whoop");
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string>();
  const load = useCallback(() => op<WhoopStatus>("whoop.status").then(setSt, (e) => setErr(errText(e))), []);
  useEffect(() => {
    load();
  }, [load]);
  useEffect(() => {
    if (st?.state !== "pending") return;
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, [st, load]);
  const run = async (f: () => Promise<void>) => {
    setErr(undefined);
    setNote(undefined);
    setBusy(true);
    try {
      await f();
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
      load();
    }
  };
  const connect = () =>
    run(async () => {
      const b = await op<{ url: string; redirect: string }>("whoop.begin", { client_id: id, client_secret: secret, open: true });
      setRedirect(b.redirect);
      setSecret("");
      setNote(t("WHOOP's sign-in page opened in your browser. Health finishes when you come back."));
    });
  const sync = () =>
    run(async () => {
      const r = await op<{ written: number }>("whoop.sync", { force: true });
      setNote(r.written ? t("{{count}} new readings from WHOOP.", { count: r.written }) : t("No new WHOOP data."));
    });
  if (!st) return null;
  return (
    <div className="group form">
      <h3>WHOOP</h3>
      {st.state === "connected" ? (
        <>
          <p className="fine">
            {st.last_sync
              ? t("Connected. Health fetches new WHOOP data every hour while it's open; last time {{when}}.", { when: when(st.last_sync) })
              : t("Connected. Health fetches new WHOOP data every hour while it's open.")}
          </p>
          <div className="actions">
            <button type="button" disabled={busy} onClick={sync}>
              {busy ? t("Syncing…") : t("Sync now")}
            </button>
            <button type="button" disabled={busy} onClick={() => run(async () => void (await op("whoop.disconnect")))}>
              {t("Disconnect")}
            </button>
          </div>
        </>
      ) : st.state === "pending" ? (
        <p className="fine">{t("Waiting for the WHOOP sign-in in your browser…")}</p>
      ) : (
        <form
          className="form"
          onSubmit={(e) => {
            e.preventDefault();
            connect();
          }}
        >
          <p className="fine">
            {st.state === "reauth" ? `${t("WHOOP needs you to sign in again.")} ` : ""}
            {t("Use your own WHOOP developer app (developer.whoop.com) and add this redirect URL to it:")} <span className="mono">{redirect}</span>{" "}
            <Copy text={redirect} label={t("Copy")} />
          </p>
          <label>
            {t("Client ID")}
            <input value={id} onChange={(e) => setId(e.target.value)} autoComplete="off" spellCheck={false} />
          </label>
          <PasswordField label={t("Client Secret")} value={secret} onChange={setSecret} auto="off" />
          <div className="actions">
            <button type="submit" className="primary" disabled={busy || !id.trim() || !secret.trim()}>
              {t("Connect WHOOP")}
            </button>
          </div>
        </form>
      )}
      {note && <p className="fine" role="status">{note}</p>}
      <Problem text={err} />
    </div>
  );
}

// ---- Profile (web personal.tsx): sex and birth month kept on this computer; height and weight are the latest
// readings, and changing one here records a new reading -------------------------------------------------------

const HEIGHT = "loinc:8302-2";
const WEIGHT = "loinc:29463-7";
type Personal = { sex?: "Male" | "Female"; age?: number; birth?: string };
type ProfileField = "sex" | "birth" | "height" | "weight";

const sexLabel = (s?: string) => (s === "Male" ? t("Male") : s === "Female" ? t("Female") : undefined);
const birthLabel = (b: string) =>
  new Intl.DateTimeFormat(i18nLocale(), { year: "numeric", month: "long", timeZone: "UTC" }).format(Date.UTC(Number(b.slice(0, 4)), Number(b.slice(5, 7)) - 1, 1));

function ProfilePage() {
  const [me, , reloadMe] = useOp(() => op<Personal>("profile.get"), []);
  const [latest, , reloadLatest] = useOp(() => op<Point[]>("obs.latest"), []);
  const [edit, setEdit] = useState<ProfileField>();
  const latestOf = (code: string) => {
    const v = latest?.find((p) => p.metric === code)?.value;
    return typeof v === "number" ? v : undefined;
  };
  const h = latestOf(HEIGHT);
  const w = latestOf(WEIGHT);
  const unset = <span className="fine">{t("Not set")}</span>;
  // as the web shows them: one decimal at most
  const one = (v: number) => (Math.round(v * 10) / 10).toLocaleString(i18nLocale(), { maximumFractionDigits: 1 });
  const rows: { k: ProfileField; label: string; value: ReactNode }[] = [
    { k: "sex", label: t("Sex"), value: sexLabel(me?.sex) ?? unset },
    { k: "birth", label: t("Birth month"), value: me?.birth ? `${birthLabel(me.birth)} · ${t("{{count}} years old", { count: me.age ?? 0 })}` : unset },
    { k: "height", label: t("Height"), value: h !== undefined ? `${one(h * 100)} cm` : unset },
    { k: "weight", label: t("Weight"), value: w !== undefined ? `${one(w)} kg` : unset },
  ];
  return (
    <section className="page">
      <h2>{t("Profile")}</h2>
      <p className="lede">{t("Kept on this computer only. Height and weight are your latest readings; changing them here records a new one.")}</p>
      <ul className="rows-card">
        {rows.map((x) => (
          <li key={x.k}>
            <button type="button" className="row-btn" onClick={() => setEdit(x.k)}>
              <span className="row-l">
                <strong>{x.label}</strong>
              </span>
              <span className="row-v">{x.value}</span>
              <span className="row-chev" aria-hidden="true">
                ›
              </span>
            </button>
          </li>
        ))}
      </ul>
      {edit && (
        <Sheet label={rows.find((x) => x.k === edit)!.label} close={() => setEdit(undefined)}>
          <ProfileEdit
            field={edit}
            me={me ?? {}}
            done={() => {
              setEdit(undefined);
              reloadMe();
              reloadLatest();
            }}
          />
        </Sheet>
      )}
    </section>
  );
}

function ProfileEdit({ field, me, done }: { field: ProfileField; me: Personal; done: () => void }) {
  const [year, setYear] = useState(me.birth?.slice(0, 4) ?? "");
  const [month, setMonth] = useState(me.birth ? String(Number(me.birth.slice(5, 7))) : "");
  const [value, setValue] = useState("");
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  const now = new Date();
  const run = async (f: () => Promise<unknown>) => {
    setErr(undefined);
    setBusy(true);
    try {
      await f();
      done();
    } catch (e) {
      setErr(errText(e));
      setBusy(false);
    }
  };
  const measure = (metric: string, unit: string, min: number, max: number) => {
    const v = parseNumber(value);
    if (!Number.isFinite(v) || v < min || v > max) return setErr(t("Type a number between {{min}} and {{max}}.", { min, max }));
    const d = new Date();
    run(() => op("manual.enter", { entries: [{ metric, value: v, unit, at: { ms: d.getTime(), tz_min: -d.getTimezoneOffset() } }] }));
  };
  const title = { sex: t("Sex"), birth: t("Birth month"), height: t("Height"), weight: t("Weight") }[field];
  return (
    <section className="record">
      <h2>{title}</h2>
      {field === "sex" && (
        <>
          <ul className="rows-card" role="radiogroup" aria-label={t("Sex")}>
            {(["Female", "Male"] as const).map((s) => (
              <li key={s}>
                <button
                  type="button"
                  className="row-btn"
                  role="radio"
                  aria-checked={me.sex === s}
                  disabled={busy}
                  onClick={() => run(() => op("profile.set", { sex: s }))}
                >
                  <span className="row-l">
                    <strong>{sexLabel(s)}</strong>
                  </span>
                  <span className="row-check" aria-hidden="true">
                    {me.sex === s ? "✓" : ""}
                  </span>
                </button>
              </li>
            ))}
          </ul>
          {me.sex && (
            <button type="button" className="link" disabled={busy} onClick={() => run(() => op("profile.set", { sex: null }))}>
              {t("Clear")}
            </button>
          )}
        </>
      )}
      {field === "birth" && (
        <form
          className="form"
          onSubmit={(e) => {
            e.preventDefault();
            if (!year || !month) return setErr(t("Choose a year and a month."));
            run(() => op("profile.set", { birth: `${year}-${month.padStart(2, "0")}` }));
          }}
        >
          <label>
            {t("Year")}
            <select value={year} onChange={(e) => setYear(e.target.value)} required>
              <option value="">{t("Choose…")}</option>
              {Array.from({ length: now.getFullYear() - 1900 + 1 }, (_, i) => now.getFullYear() - i).map((y) => (
                <option key={y} value={y}>
                  {y}
                </option>
              ))}
            </select>
          </label>
          <label>
            {t("Month")}
            <select value={month} onChange={(e) => setMonth(e.target.value)} required>
              <option value="">{t("Choose…")}</option>
              {Array.from({ length: 12 }, (_, i) => i + 1).map((m) => (
                <option key={m} value={m}>
                  {new Intl.DateTimeFormat(i18nLocale(), { month: "long", timeZone: "UTC" }).format(Date.UTC(2000, m - 1, 1))}
                </option>
              ))}
            </select>
          </label>
          <Problem text={err} />
          <div className="actions">
            <button type="submit" className="primary" disabled={busy}>
              {t("Save")}
            </button>
            {me.birth && (
              <button type="button" className="link" disabled={busy} onClick={() => run(() => op("profile.set", { birth: null }))}>
                {t("Clear")}
              </button>
            )}
          </div>
        </form>
      )}
      {(field === "height" || field === "weight") && (
        <form
          className="form"
          onSubmit={(e) => {
            e.preventDefault();
            if (field === "height") measure(HEIGHT, "cm", 30, 272);
            else measure(WEIGHT, "kg", 1, 650);
          }}
        >
          <label>
            {field === "height" ? t("Height (cm)") : t("Weight (kg)")}
            <input inputMode="decimal" value={value} onChange={(e) => setValue(e.target.value)} autoFocus required />
          </label>
          <p className="fine">{t("Saved as a new reading now, entered by you.")}</p>
          <Problem text={err} />
          <div className="actions">
            <button type="submit" className="primary" disabled={busy}>
              {t("Save")}
            </button>
          </div>
        </form>
      )}
      {field === "sex" && <Problem text={err} />}
    </section>
  );
}

type LinkEntry = { id: string; dir: "gave" | "got"; at: number; check: string };

/** Link history (owner, 2026-11-22: a local record of the links this device completed; list only, as the web's
 *  连接记录). Newest first, after this device itself. */
function LinkHistoryList() {
  const [h] = useOp(() => op<{ state: "ok"; entries: LinkEntry[] } | { state: "unreadable" }>("link.devices"), []);
  if (!h) return null;
  return (
    <>
      <h3 className="rows-head">{t("Link history")}</h3>
      <ul className="rows-card" aria-label={t("Link history")}>
        <li className="row-static">
          <Icon k="account" />
          <span className="row-l">
            <strong>{t("This device")}</strong>
          </span>
          <span className="tag">{t("This one")}</span>
        </li>
        {h.state === "unreadable" ? (
          <li className="row-static fine">{t("Can't read the link history")}</li>
        ) : h.entries.length === 0 ? (
          <li className="row-static fine">{t("No links recorded on this device")}</li>
        ) : (
          h.entries.map((x) => (
            <li key={x.id} className="row-static">
              <Icon k="account" />
              <span className="row-l">
                <strong>{x.dir === "gave" ? t("Handed over from this device") : t("Received from another device")}</strong>
                <span className="fine">{t("{{date}} · Check code {{code}}", { date: when(x.at), code: x.check })}</span>
                {x.dir === "gave" && <span className="fine">{t("Whether the other device confirmed, this one can't know")}</span>}
              </span>
            </li>
          ))
        )}
      </ul>
      <p className="fine">
        {t("This is this device's own record, kept from this version on. If the other device erases its data or hands the account on, this one won't know. A device can't yet be stopped from using your health identity.")}
      </p>
    </>
  );
}
