// Display rules shared by every page (ported from the web model.ts): units as people write them,
// durations as hours and minutes, censored values with their comparator.

const UNITS: Record<string, string> = {
  "mm[Hg]": "mmHg", Cel: "°C", "[lb_av]": "lb", "[oz_av]": "oz", "[stone_av]": "st", "[in_i]": "in", "[ft_i]": "ft",
  "[mi_i]": "mi", "[degF]": "°F", "[IU]/L": "IU/L", "m[IU]/L": "mIU/L", "u[IU]/mL": "uIU/mL", "meq/L": "mEq/L",
  "1": "", "/uL": "/µL", "/min": "/min", "mL/min/{1.73_m2}": "mL/min/1.73m²",
};
const SUP = "⁰¹²³⁴⁵⁶⁷⁸⁹";
export function unitLabel(u: string): string {
  if (u in UNITS) return UNITS[u] as string;
  return u
    .replace(/\{[^}]*\}/g, "")
    .replace(/[[\]]/g, "")
    .replace(/10\*(\d+)/g, (_, n: string) => `10${[...n].map((d) => SUP[Number(d)]).join("")}`)
    .replace(/(^|\/)u(L|l)$/, "$1µ$2");
}

const PER_MS: Record<string, number> = { ms: 1, s: 1000, min: 60_000, h: 3_600_000 };

export function duration(ms: number): string {
  const m = Math.round(ms / 60_000);
  const h = Math.floor(m / 60);
  return h ? `${h} h ${String(m % 60).padStart(2, "0")}` : `${m} min`;
}

/** What people read (the web's model.ts): whole numbers from 100, one decimal from 10, three below. Lab results
 *  keep every printed decimal (up to 6): 1.025 and 1.03 are different results. */
export function num(v: number, lab = false): string {
  const a = Math.abs(v);
  const digits = lab ? 6 : a >= 100 ? 0 : a >= 10 ? 1 : 3;
  return v.toLocaleString(undefined, { maximumFractionDigits: digits });
}

/** Lab-like groups show values as printed. */
export const isLabGroup = (g?: string) => g === "lab" || !!GROUPS.find((x) => x.key === g)?.sensitive;

export const PER_MS_UNITS = new Set(["ms", "s", "min", "h"]);
/** A duration unit's value as hours and minutes (`1 h 30`), else undefined. */
export function asDuration(v: number, unit: string, minuteUp = false): string | undefined {
  const per = PER_MS[unit];
  if (!per || (minuteUp && Math.abs(v * per) < 60_000)) return undefined;
  return duration(v * per);
}

export interface Shown {
  value: string;
  unit: string;
}

export function shown(p: Pick<import("./api").Point, "agg" | "value" | "unit" | "start_ms" | "end_ms" | "comparator"> & { group?: string }): Shown {
  if (p.agg === "interval") return { value: p.end_ms ? duration(p.end_ms - p.start_ms) : "", unit: "" };
  if (p.value === null) return { value: "no reading", unit: "" };
  if (typeof p.value === "string") return { value: p.value, unit: "" };
  const per = PER_MS[p.unit];
  if (per && !p.comparator && Math.abs(p.value * per) >= 60_000) return { value: duration(p.value * per), unit: "" };
  return { value: `${p.comparator ?? ""}${num(p.value, isLabGroup(p.group))}`, unit: unitLabel(p.unit) };
}

/** The number a daily row is drawn and read by. */
export function dayValue(agg: string, d: import("./api").Day): number | null {
  if (agg === "interval") return d.seconds === null ? null : d.seconds / 3600;
  if (agg === "cumulative") return d.sum;
  return d.mean ?? null;
}

export function dayUnit(agg: string, unit: string): string {
  return agg === "interval" ? "h" : unitLabel(unit);
}

/** Calendar days between the reading's local day and today (not 24-hour periods). */
export function ago(ms: number, now = Date.now()): string {
  const midnight = (t: number) => new Date(new Date(t).toDateString()).getTime();
  const d = Math.round((midnight(now) - midnight(ms)) / 86_400_000);
  if (d <= 0) return "today";
  if (d === 1) return "yesterday";
  if (d < 60) return `${d} days ago`;
  return shortDate(ms);
}

export const GROUPS: { key: string; label: string; sensitive: boolean }[] = [
  { key: "body", label: "Body and vitals", sensitive: false },
  { key: "cardio", label: "Heart and recovery", sensitive: false },
  { key: "sleep", label: "Sleep", sensitive: false },
  { key: "activity", label: "Activity", sensitive: false },
  { key: "lab", label: "Routine lab", sensitive: false },
  { key: "infectious", label: "Infectious disease tests", sensitive: true },
  { key: "tumor", label: "Tumour markers", sensitive: true },
  { key: "repro", label: "Reproductive health", sensitive: true },
  { key: "custom", label: "Your own lab items", sensitive: true },
  { key: "lab_other", label: "Other lab tests", sensitive: true },
  { key: "other", label: "Other", sensitive: true },
  { key: "derived", label: "Computed by agents", sensitive: true },
];
export const groupLabel = (k: string) => GROUPS.find((g) => g.key === k)?.label ?? k;

export const SOURCE_KIND: Record<string, string> = {
  manual: "Entered by you",
  apple_health: "Apple Health",
  whoop: "WHOOP",
  ocr: "Lab report photo",
  lab: "Lab",
  staff: "Clinic staff",
  agent: "Agent proposal",
  derived: "Computed",
  file: "File",
  room: "Sleep room",
};
export const sourceKind = (c: string) => SOURCE_KIND[c] ?? c;

/** The local calendar day (daily rows are keyed by the local day), as YYYY-MM-DD. */
export const day = (ms: number) => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};
/** Local midnight of a YYYY-MM-DD day (Date.parse would read it as UTC). */
export const dayMs = (s: string) => {
  const [y, m, d] = s.split("-").map(Number);
  return new Date(y!, m! - 1, d!).getTime();
};
/** A short date: "Mar 3", with the year when it isn't this year. */
export const shortDate = (ms: number) => {
  const d = new Date(ms);
  return d.toLocaleDateString([], d.getFullYear() === new Date().getFullYear() ? { month: "short", day: "numeric" } : { dateStyle: "medium" });
};
/** A reading's time in the zone it was measured in (a trip abroad shows the local clock there), as the CSV does. */
export const when = (ms: number, tz?: number) =>
  tz === undefined
    ? new Date(ms).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })
    : new Date(ms + tz * 60_000).toLocaleString([], { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" });

/** The date alone, in the zone it was measured in (entries made for a day, not a moment). */
export const dateIn = (ms: number, tz: number) => new Date(ms + tz * 60_000).toLocaleDateString([], { dateStyle: "medium", timeZone: "UTC" });

/** What a person should read for an error code: what happened and what to do. Unknown codes keep the core's message. */
export function explain(code: string, message = ""): string {
  const known: Record<string, string> = {
    pack_auth: "Wrong password, or the file was changed after it was saved.",
    pack_format: "This is not a Health backup file.",
    pack_truncated: "The backup file is incomplete. Copy it again and retry.",
    pack_container: "The backup file is damaged.",
    backup_db: "The backup's contents are not a Health database.",
    backup_dup: "The backup file is damaged (an entry appears twice).",
    not_apple: "This is not an Apple Health export. In the Health app on iPhone: your picture, then Export All Health Data.",
    bad_whoop: "This is not a WHOOP data file Health can read.",
    bad_row: "A line in the file is not a reading Health understands.",
    no_file: "The file can't be found. It may have been moved.",
    exists: "A file with that name already exists. Choose another name.",
    weak_password: "Use at least 8 characters.",
    blob_dup: "This file was already imported: it is under Sources, or under Waiting for you if it hasn't been checked yet.",
    unit_required: "Choose the unit your device showed.",
    unit_unknown: "That unit doesn't fit this measure.",
    rejected: "The value doesn't fit this measure.",
    import_failed: "The file couldn't be read to the end. If it is in iCloud, download it first.",
    bad_passphrase: "That passphrase is not right.",
    unavailable: "The health core is restarting. Try again in a moment.",
    no_ocr_component: "Reading lab reports needs the OCR component. Install it below.",
    unsupported: "Health can't read this kind of file. Use a photo (JPEG, PNG) or a PDF; on iPhone, share HEIC photos as JPEG.",
    unreadable: "The file couldn't be read. It may be damaged.",
    password: "This PDF is protected by a password. Save a copy without it and try again.",
    too_big: "The file is larger than 40 MB.",
    empty: "No lab results were found in this file. Try a sharper, straight photo of the table.",
    ocr_crashed: "The reader stopped unexpectedly. Your data is untouched; try again or use a smaller file.",
    duplicate_target: "Two items point to the same measure. Change or skip one of them.",
    nothing_to_save: "Choose a measure for at least one item.",
    same_day_report: "Another report of the same day already has a value for some of these measures. Skip those items, or set a different date.",
    date_required: "Set the day the sample was taken.",
    ocr_component: "The recognition models on this computer are damaged. Install them again below.",
    bad_tz: "The time zone isn't valid.",
    network: "The download failed. Check the internet connection and try again.",
    integrity: "A downloaded file wasn't the expected one and was discarded. Try again later.",
  };
  return known[code] ?? (message || code);
}

export type ImportKind = "apple" | "backup" | "whoop" | "rows" | "report";

/** Photos and PDFs of lab reports (HEIC isn't read: the phone can share it as JPEG). */
export const REPORT_EXTENSIONS = ["jpg", "jpeg", "png", "webp", "tif", "tiff", "bmp", "gif", "pdf"];

/** What a file is, by its name. */
export function importKind(path: string): ImportKind | null {
  const p = path.toLowerCase();
  if (REPORT_EXTENSIONS.some((e) => p.endsWith(`.${e}`))) return "report";
  if (p.endsWith(".zuhealth")) return "backup";
  if (p.endsWith(".zip")) return "apple";
  if (p.endsWith(".jsonl")) return "rows";
  if (p.endsWith(".json")) return "whoop";
  return null;
}

export const KIND_LABEL: Record<ImportKind, string> = {
  apple: "Apple Health export",
  backup: "Health backup",
  whoop: "WHOOP data",
  rows: "Readings file (JSON lines)",
  report: "Lab report (photo or PDF)",
};

export const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;
