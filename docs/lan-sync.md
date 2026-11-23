# Automatic full sync between devices on the same LAN

Status: draft v0.3 (2026-11-23), revised after two rounds of independent review (Fable 5.2). Not implemented.

Owner decisions so far:

- No large architecture change; sync must not depend on the person exporting and importing backups.
- Devices on the same local network (web, desktop now; Android and iOS later) sync automatically.
- **Every device holds the complete data.** Any one device alone must be fully usable, so there are no partial
  replicas. This supersedes the "phones keep 90 days" line in the DB design §5.3.

## 1. Summary

Two devices that hold the same health account (the same AMK, given by "Link a new device", protocol 3.5) meet
through the backend, which relays a few encrypted messages so they can open a WebRTC data channel to each other
over private addresses. Over the channel they compare an **inventory**: every import with its status and a
fingerprint of the rows it owns now, plus corrections, synced settings and missing files. Each side asks for what
differs. Imports travel as ordinary backup packs (`zu-health-export` v1) and are merged with the restore rules
(`health_core::merge`, the web's `db/merge.ts`), after the fixes those rules need once merging runs all the time
(§7). With the fixes, the merge is a join (per row key, the largest non-erased uid wins; tombstones only grow), so
repeated pairwise exchanges converge every device to the same data regardless of order.

New: a transport (WebRTC + signalling), the inventory, a subset export, a small sync state, eight merge fixes (six shared, two web-only).
Reused: account and pairing (linking), identities (`import.uid`, `metric.code`, `source(connector,
external_id)`), the pack format, `backup::import` and its untrusted-input checks, erase tombstones, daily rebuild.

## 2. Goals and non-goals

Goals

1. G1 Every linked device converges to identical data: imports (done, review, erased tombstones), their
   observations, genotypes and report text, corrections, the files those reference (blobs), and the synced
   settings in §8. Metric aliases are merged but may differ (§7.6).
2. G2 Automatic: once devices are linked, sync runs whenever two of them are on the same network and open.
   The first sync with each new peer, and any sync that would erase a lot, asks first (§10).
3. G3 Data goes directly between the devices over private addresses (§5.4). The backend sees neither content nor
   local addresses.
4. G4 Works across implementations: the web (TypeScript) and the Rust core (desktop now, mobile later), each on
   SQLite with the same schema.
5. G5 Interruptible: a dropped connection loses at most the batch in flight.

Non-goals (now)

- Sync across the internet (TURN, a server-side log). The sync layer is transport-agnostic so one can be added.
- Devices that are never on the same network at the same time. Data reaches them through a device that meets both.
- Removing a device (AMK rotation). Any AMK holder can sync, as any AMK holder can already read the inbox.
- Syncing device-local state: WHOOP tokens, agent grants and the access log, link history, the AMK.

## 3. Architecture

```
          ┌──────────── sync engine (pure: no network, no clock) ────────────┐
          │ inventory · plan · subset export · merge (existing + §7 fixes)    │
          └───────────────▲───────────────────────────────▲──────────────────┘
                          │ framed messages                │ framed messages
                 ┌────────┴────────┐               ┌───────┴──────────┐
 desktop/mobile  │ transport: str0m │ private addrs │ RTCPeerConnection│  web
 (Rust)          │ (+ mDNS if need) │◀═════════════▶│ (browser)        │  (TypeScript)
                 └────────┬────────┘   WebRTC DC    └───────┬──────────┘
                          │ signalling (encrypted, few KB)  │
                          └──────────▶ backend relay ◀──────┘
```

| Piece | Crate | Notes |
|---|---|---|
| inventory, fingerprints, subset export, merge fixes | `health-core` | no network, clock as argument |
| sync engine (messages, planning, negative cache) | `health-api` (`sync.rs`), owner only, like the `account.rs` net ops | the profile is locked only for short steps |
| signalling client, WebRTC (`str0m`), mDNS if needed, UDP sockets | `health-connect` | the only networking crate; add `str0m` and the mDNS crate to the banned list in `scripts/check-no-net.sh` for every other crate |
| the running loop | `health serve` | sync runs only while serve runs (the app is open). CLI: `health sync status` |

`str0m` is a sans-IO WebRTC implementation in pure Rust (MIT/Apache-2.0) with no async runtime, which keeps
decision 24 (no tokio). Its crypto backend is a feature choice; it and any mDNS crate must pass `cargo deny`.
The web side uses the browser's `RTCPeerConnection` and its existing `merge.ts` and `pack.ts`.

## 4. Keys and identifiers

Derived from the AMK with HKDF-SHA256, salt `health-amk-v1` (as protocol 0.2). Every linked device computes the
same values; the backend can compute none.

| Name | info | Use |
|---|---|---|
| `K_d` | `lan-sync/key/v1/<UTC day YYYY-MM-DD>` | AES-256-GCM key for that day's signalling |
| `R_d` | `lan-sync/rid/v1/<UTC day YYYY-MM-DD>` | first 16 bytes, base32: that day's rendezvous id |

Both rotate daily, so the backend can't link one account's presence across days. For 10 minutes after midnight
UTC devices also poll and accept the previous day's id and key, and for 2 minutes before it the next day's (a peer
whose clock runs ahead).

**Device id**: 16 random bytes made once per installation and kept in the vault (the web: in its own store).
It isn't synced and isn't in backups. Several tabs of one browser share one device id; only the tab holding a
Web Lock (`health-lan-sync`) runs sync.

## 5. Signalling

### 5.1 Backend endpoint (new, small)

Unauthenticated like the link relay, rate-limited per IP:

- `POST /h/1/lan/<rid>`, body `{"ct": "<b64>"}` (≤ 8 KiB decoded) → `{"seq": n}`.
- `GET /h/1/lan/<rid>?after=<seq>&wait=30` → `{"items": [{"seq", "ct"}], "more": bool}`. A long poll, as
  protocol §5.1 does for the gateway: it answers as soon as something newer arrives, or after `wait` seconds.
  Messages live 120 s; at most 64 per rid are kept (oldest dropped).

The backend never learns which account a rid belongs to and can't read or forge messages. It can drop, delay or
replay them: §5.3 covers replay; dropping only delays sync.

Alternative considered: reusing `/h/1/link/<channel>` (one message per channel, read-and-delete, first writer
wins). It needs a channel per message, and several devices polling from behind one public IP run into its per-IP
limit. **Owner decision** (§13).

### 5.2 Messages

Each message is `AES-256-GCM(K_d, nonce, JSON, aad = rid)` with a random 12-byte nonce; the JSON carries
`{"v": 1, "t": <ms>, "id": <16 random bytes b64u>, "from": <device id>, …}`:

| type | Extra fields | Sent |
|---|---|---|
| `hello` | `name` ("MacBook · Health 0.4", "Chrome on Windows"), `proto: [1]` | on start, after a network change, and every 5 minutes |
| `offer` | `to`, `sdp` | by the device with the smaller id, on a `hello` from a device it has no channel to |
| `answer` | `to`, `re` (the offer's `id`), `sdp` | in reply |

One long poll runs while the device is open (a web page: while visible). A device alone on the network costs one
request every 30 s. A device that sees a `hello` from an unknown peer whose id is smaller than its own sends its own
`hello` at once (at most once per 30 s per peer), so the pair forms in seconds rather than at the next periodic
`hello`.

### 5.3 Checks on receipt

- It must decrypt under `K_d` (or the previous day's key near midnight) with this rid as AAD; otherwise it is ignored.
- `|now − t| ≤ 120 s` and `id` unseen in the last 10 minutes; otherwise it is ignored. Repeated rejections for skew mean this
  clock is off, and the UI says so ("this computer's clock is off by more than 2 minutes", as for the backend).
- `from` is not this device; `to`, when present, is this device; an `answer`'s `re` names an offer this device sent.
- Only `host` candidates are used: a private address (RFC 1918, 169.254/16, fe80::/10, fc00::/7) or an mDNS
  `.local` name. Other candidates are stripped before use.

The SDP, and with it the DTLS certificate fingerprint, is authenticated by `K_d`, so only an AMK holder can produce
an offer or answer this device accepts. The DTLS handshake must then be checked against that fingerprint (browsers
do; S0 confirms str0m does). So the peer on the channel holds the AMK, and neither the backend nor a party on the
local network can sit in between. A replayed message within 120 s carries the legitimate fingerprints and can't
be used to impersonate a device.

### 5.4 ICE

- No STUN or TURN servers (`iceServers: []`): host candidates only. The STUN fallback (§14.3) is the one exception.
- ICE can still learn peer-reflexive candidates from any address that knows the credentials. Before sending
  anything, each side checks that the selected pair's remote address is private (§5.3 list) or loopback, else it
  closes. Private includes VPN tunnels; that is accepted ("direct, over private addresses").
- Browsers hide local IPs behind mDNS names. The Rust side may not need to resolve them: the browser connects to
  the Rust side's real host candidate, and the Rust side learns the browser's address as peer-reflexive. S0 tests this
  first; an mDNS resolver is added only if it fails.
- If no pair connects within 15 s (client isolation on guest Wi-Fi, separate subnets, a firewall), that peer backs
  off for 5 minutes and the UI says "On the same network but can't connect directly".

## 6. Sync protocol (over the data channel)

One reliable, ordered data channel `health-sync/1`. **Every** message is framed: a 1-byte type, then JSON or
bytes, split into frames of ≤ 16 KiB and reassembled (control messages can be megabytes, e.g. a large
inventory). Back-pressure keeps `bufferedAmount` under 1 MiB.

### 6.1 Inventory

The inventory is a list of lines, in this order, each sorted bytewise (UTF-8):

1. `i <uid> <status> <blob> <fp>` for every import with status `done`, `review` or `erased`. `blob` is the
   sha it holds or `-`. `fp` is the import's **fingerprint** for `done` (§6.2), else `-`: a `review` with the same
   uid on two devices comes from the same signed result, and Rust and JS serialise its `meta` differently, so it is
   not compared. `running` and `failed` imports are left out (`failed` is erased at once, `db.rs`).
2. `c <code> <connector> <external_id> <start_ms> <at> <value>`: every correction.
3. `s <key> <at> <by> <value>`: every synced setting (§8).
4. `b <sha>`: every blob referenced by a non-erased import or a waveform row that this device **lacks**.

The **digest** is the SHA-256 of lines 1–3. Lines 4 don't enter it (a missing file is a want, not a difference in
data); `hello` carries their count as `lacks` instead.

Values are written exactly, so Rust and JS agree byte for byte: integers in decimal; reals as the 16 hex digits of
their IEEE-754 bits; text as `<byte length>:<bytes>`; null as `n`. Shared test vectors pin this (S5).

### 6.2 Fingerprints

A `done` import's fingerprint is `<n>:<h>`: `n` the number of rows it owns now and `h` the sum, modulo 2^256, of
one SHA-256 per row. The sum doesn't depend on order, so no sort is needed. Rows are:

- every observation whose `import_id` is the import: `o ‖ metric code ‖ connector ‖ external_id ‖ start_ms ‖
  end_ms ‖ tz_min ‖ typeof(value) ‖ value ‖ detail`, encoded as §6.1;
- its genotypes: `g ‖ chrom ‖ pos ‖ rsid ‖ genotype`;
- its report text: `t ‖ text`.

Why rows and not only uid and status: what an import owns changes after it is written. A newer import takes rows
over. Erasing that newer import then deletes those rows on the device that erased it, but not on a device that only
receives the tombstone. Equal `(uid, status)` would hide different data. Different fingerprints make both sides want
the import; the merge of `done` with `done` unites the rows, and every device ends with what any device still holds
under a non-erased import (§14.5).

Computing and caching:

- **All at once.** One scan of `observation` grouped by `import_id` (plus `genotype` and `report_text`) computes
  every fingerprint. Span lookups are not used: WHOOP's hourly imports cover the same few days, so per-import span
  scans would revisit the same rows thousands of times.
- **In memory only.** The cache lives in serve and is computed when serve starts; nothing is persisted, so a CLI
  write, a restore or a crash can't leave it stale.
- **Invalidated conservatively.** When an import is written, erased or merged, the cache drops it and every import
  of the same source whose span overlaps it on a shared metric (from `import.meta`), since only those can gain or
  lose rows. A dropped entry is recomputed by the next scan.
- The web does the same in its worker.

### 6.3 Exchange

```
A ⇄ B  hello {proto: 1, device, digest, lacks}
       equal digests and no lacks on either side → done ("up to date")
A ⇄ B  inventory (framed)
A ⇄ B  want {imports: [uid…], corrections: bool, settings: [key…], blobs: [sha…]}
A ⇄ B  tombstones first (one small pack), then packs in uid order (§6.5), then blobs
A ⇄ B  done {digest}
```

Both sides pull at once. While the channel is open, any local change (serve's change event; the web's DB change
notification) sends a new `hello`. If the two digests were equal before, it also sends `changed`, holding only the
inventory lines added or removed since then, and the peer answers with its own. Otherwise every manual entry
would resend a megabyte inventory both ways.

### 6.4 What a side wants

| local \ remote | done | review | erased |
|---|---|---|---|
| absent | want | want | want (tombstone) |
| done | want if `fp` or `blob` differs | – | want |
| review | want (R2) | – | want |
| erased | – | – | – |
| running | – | – | – |

- Corrections are wanted whole when any `c` line differs.
- A setting is wanted when its `s` lines differ and the remote one is larger by `(set_at, set_by, value)`. That
  comparison is total, so two unstamped values also converge. The values travel in a `settings` message.
- A blob is wanted when listed as lacking here and present there.
- **Negative cache, one general rule.** After merging a wanted import, if the local fingerprint still differs from the
  remote one, `(peer, uid, local fp, remote fp)` is cached and the import isn't wanted again until either changes.
  The same applies to corrections, keyed by a hash of each side's `c` section. This covers everything the merge
  refuses or can't apply: an implausible uid, a correction from the future, a skipped metric (R4), and anything not
  foreseen. Without it a session would never end. The cache lives in the sync state, and the UI shows "partly
  synced" while it is non-empty for a peer.

### 6.5 Packs

- A batch is a set of wanted imports adding up to about 50,000 rows (observations and genotypes). One larger import
  goes alone, split by span into packs of about 50,000 rows each, in `(metric code, start_ms)` order. Every part
  carries the same uid and merges through the `done`/`done` branch, and the fingerprint shows when the import is
  complete.
  - **Resume:** `want` carries the last `(metric code, start_ms)` received for an import that was split, so an
    interrupted 1M-row import continues instead of restarting.
  - **Shown as incomplete:** the sync state marks such an import, and the UI shows it as "syncing" until its
    fingerprint matches.
  - **Accepted cost:** an import that differs by one row is resent whole, in both directions, because both want it.
    That is acceptable for typical imports. S0 measures the overlapping Apple Health case; if it is too costly, a
    later version sends per-span fingerprints.
- The sender builds a **subset** plain database (`merge::export_subset`): the listed imports and their rows,
  selected by span over the primary key, never by `import_id`, which has no index; their genotypes and report text;
  every metric, alias and source they reference; when asked, the whole correction table and every metric and
  source the corrections reference.
- It is wrapped in a pack with a random 32-byte password sent in the `pack` control message and `iter = 1`
  (accepted by both readers), plus the `blobs/<sha>` entries asked for. The pack layer adds nothing over DTLS.
  It is kept because both implementations already restore packs through one well-tested path
  (`backup::import`, the web's import flow), and its cost at `iter = 1` is negligible.
- The receiver writes the pack into the profile's `tmp/` and runs `backup::import`, which already treats it as
  untrusted (schema comparison, `quick_check`, `trusted_schema = OFF`, plausible uids, no future corrections).

Limits: the Rust side refuses a pack over 2 GiB; the web buffers a pack in memory (decision 8), so its packs are
capped at 256 MiB and it asks for smaller batches. An inventory over 500,000 lines ends the session with a logged
code.

## 7. Merge fixes (with shared parity fixtures)

Each change lands in the Rust core **and** the web's `merge.ts`, with a fixture both must pass, because behaviour
must match the web (CLAUDE.md).

- **R1 Equal-uid rows are decided by value.** The merge applies the incoming row when its uid is `>=` the current
  one, so for the same uid each side takes the other's row and they swap. In the merge (not in `write.rs`, where a
  later row of the same local import must still overwrite), the condition becomes
  `uid_new > uid_old OR (uid_new = uid_old AND newer(new, old))`. `newer` compares, in order,
  `typeof(value)`, `value`, `end_ms`, `tz_min`, `detail`, each with a NULL-safe step: `gt(a, b) := (b IS NULL AND
  a IS NOT NULL) OR a > b` and `eq(a, b) := a IS b`. A plain row-value comparison returns NULL on a NULL
  component, and `typeof` first keeps 5 and 5.0 apart, as §6.1 does. The same rule decides genotypes at the same
  position (today `INSERT OR IGNORE`): the larger `(genotype, rsid)`, NULL-safe. For the same import, `meta` spans
  unite and the larger report text wins.
- **R2 A confirmed review reaches devices still holding it as review.** Today a local `review` meeting a remote
  `done` is left alone (decisions, Known gaps). Rule: the local import becomes `done` with the remote rows, spans
  and text, as for a new import. The duplicate-blob check excludes the import itself, or it would drop its own file.
- **R3 Source name and priority converge.** Merge keeps the local name and priority of a known source, so edits
  never propagate and daily values differ (daily elects by priority when `v_daily` is read). Rule: an edit writes
  `{"set_at": <hlc>, "set_by": <device id>}` into `source.meta` (a JSON text column: no schema change). It is
  atomic with the edit and travels in packs. On merge, the larger `(set_at, set_by)` wins name and priority; an
  absent `set_at` counts as 0, and between two absent ones the larger `(priority, name)` wins, so defaults also
  converge.
- **R4 Metric conflicts stay visible.** A metric whose unit or agg differs (the same `x:ocr/…` or `x:derived/…` code
  made on two devices, or catalogs of two app versions) is skipped, while its import is still inserted as `done`
  without those rows. The fingerprint then differs, the import goes into the negative cache, and the UI says
  "N measures differ between your devices and aren't synced", listing them.
- **R5 Merged corrections rebuild daily.** Today only touched imports are rebuilt (Known gaps), so the daily values on
  the receiving side go stale. Rule: collect the `(metric, source, day)` of every correction that applied, and
  rebuild those.
- **R6 One owner per file.** The unique index on `import.blob` lets only one import hold a file. In `done`/`done`, a
  non-NULL blob wins over NULL. When the same file is held by two imports, the one with the **smaller uid** keeps it:
  the merge first sets the larger one's blob to NULL, then the smaller one's, an order the unique index allows. So every
  device agrees, and erasing the holder removes the file everywhere alike. The blob is in the `i` line, so a
  difference is seen even where the fingerprints are equal.
- **R7 The web erases emptied manual imports** (decisions 16 and 42, web change only). The web keeps a superseded
  manual import `done` and empty. Under union healing, an edit followed by a remove on the web would bring the
  removed value back from any device still holding the old import. Parity fixture: edit, then remove, on the web,
  then sync with a device that still holds the original.
- **R8 The web's span translation** (decision 41, web change only). The web runs the *local* span through the
  remote→local metric map in `done`/`done`. Where metric ids differ between devices, that corrupts its spans, and
  with them its split exports.

### 7.6 Aliases

`metric_alias` is merged with `INSERT OR IGNORE` while local confirmations replace. The same printed name can map
to different measures on two devices, which changes only future automatic suggestions, not data. Aliases are merged
but not in the inventory, and may differ.

## 8. Synced settings (outside the observation data)

| Key | Value | Stored | Clock |
|---|---|---|---|
| `profile` | birth month, sex, ethnicity | vault | `set_at`, `set_by` in the same vault write |
| `source/<connector>/<external_id>` | name, priority | `source` row | R3, in `source.meta` |
| `whoop/stopped` | when WHOOP was disconnected or its data deleted | vault | as profile |

Last writer wins by `(set_at, set_by, value)`. `set_at` is an HLC (`(ms << 16) | counter`, as corrections use). A
value more than `MAX_SKEW_MS` (1 day) in the future is refused, as merge refuses future corrections.

- **Device id.** `source_update` takes the device id so it can stamp `set_by`.
- **Upgrading.** On the version that adds sync, every source whose name or priority differs from the defaults
  (`write.rs`) gets `set_at` = now, so an edit made before sync existed isn't reverted by an unstamped default
  elsewhere.

`whoop/stopped`: deleting WHOOP's data (decision 72) syncs as tombstones, but another device still connected would
fetch the last days again an hour later and sync them back. A newer `whoop/stopped` than a device's own WHOOP
connection time disconnects WHOOP there too, saying why.

Not synced: WHOOP tokens (each refresh rotates them, so two devices sharing them lock each other out; each device
connects WHOOP itself and identical readings converge through the observation identity), agent grants and the
access log, link history, the AMK, the device id, the UI language.

## 9. Sync state (per device, never synced, never in backups)

A separate file `sync`, sealed like the vault with its own derived key. It isn't kept in the vault: the caches
and `last_seen` change often, and the vault holds the AMK and is renamed into place without a directory fsync
(Known gaps), so it should be written rarely. Contents:

- the device id;
- the negative cache;
- split-import resume points;
- `peers: {device_id: {name, last_seen, last_synced, up_to_date}}`;
- `joined` (§10);
- `paused`;
- `enabled`.

The fingerprint cache is in memory only (§6.2).

## 10. Safety: confirmations, locks, offline

Automatic merging is irreversible, so two things ask first:

- **When this device first joins**: before its first sync ever, it asks once: "Combine this device's data with
  your other devices on this network? This device has 1,234 imports." Only a newly joining device can bring the
  wrong data, such as a device linked to the wrong account by mistake. Devices already syncing don't ask again,
  and data that arrives through an already joined peer needs no new prompt.
- **Large erase**: a session asks before merging tombstones that would delete rows here, when they would delete
  more than 20 imports holding rows or 10% of this device's rows. Tombstones of imports that are already empty don't
  count: every manual edit leaves one (decision 16), and a device offline for a month must not be asked about
  routine edits. Tombstones can't be declined individually, so "Not now" pauses sync on this device. The account
  page shows "Sync paused" with a button to continue.

Rules:

- The idle and passphrase locks drop `K_d`, every channel and the session state; sync resumes after unlock.
  Sync traffic never resets the idle timer.
- `--offline` refuses signalling and LAN sync, as it refuses the other network uses.
- Sync needs only the AMK, not a registered inbox key. A device with no account (never linked) doesn't sync.

## 11. UI

On the Health account page, below the linked devices:

- A switch, "Sync on this network". On once the device is linked (owner to confirm). The first sync with each
  device still asks (§10).
- A row per device seen, with its last sync time and one of these states:
  - "Up to date"
  - "Syncing · 1,240 of 5,300 imports"
  - "Partly synced: 2 measures differ"
  - "Can't connect directly on this network"
  - "Waiting for you to allow"
  - "This computer's clock is off"
- The web also explains that the browser may clear its storage and that the data then comes back from another
  device on this network. It calls `navigator.storage.persist()`.

Changes arriving from another device refresh the readings through the usual change event.

## 12. Security and privacy

| Party | Can | Can't |
|---|---|---|
| Backend | see a daily-rotating rid, IPs, message sizes and timing, when devices are online together | read or forge signalling, learn local addresses, sit in the channel, see data |
| Someone on the local network without the AMK | see that two devices talk (UDP, mDNS) | join, read or alter: the SDP needs `K_d`, then DTLS |
| A linked device (holds the AMK) | everything a restored backup can do | bypass the merge's input checks, or erase a lot without the person being asked (§10) |

- Logs: codes, counts and durations only. Never values, uids, report text, SDP or addresses.
- Every parse of peer input is bounded: frames, reassembled message size, JSON depth, inventory lines, pack size.

## 13. Risks to verify first (S0 spike)

1. **Safari** may withhold host candidates without a capture permission. WebKit bug 189503 says it did; mDNS
   candidates came later. Current behaviour for pages with only a data channel is unverified. If it fails, the owner
   chooses: Chrome only for the web, or a STUN server.
2. **Chrome Local Network Access.** Chrome is prototyping a permission prompt for local ICE candidates (an Intent to
   Prototype in 2025 and a libwebrtc change; shipping milestone unconfirmed). Plan for a "connect to devices on your
   local network" prompt on the web page, and word the UI around it.
3. **Native prompts and firewalls**: the macOS and iOS Local Network privacy prompt (and whether ad-hoc-signed
   builds prompt again), and the Windows inbound firewall prompt for UDP.
4. **str0m** interop with Chrome, Safari and Firefox data channels: DTLS roles, fingerprint check, SCTP throughput.
   Also whether peer-reflexive learning makes an mDNS resolver unnecessary (§5.4).
5. **Scale**: on a 1M-row profile, the inventory and fingerprint time, the subset export time, the merge time per
   50,000-row batch, and how long serve's lock is held (Known gaps: long operations block). Record the results in
   `docs/perf.md`.

## 14. Owner decisions (2026-11-23)

1. Signalling uses the new `/h/1/lan` endpoint (§5.1).
2. Sync is on by default once a device is linked, with the confirmation when a device first joins (§10).
3. If S0 shows Safari can't connect with host candidates only, add a STUN fallback:
   - Try host candidates first. Only if no pair connects in 15 s, retry with STUN.
   - A pair over server-reflexive candidates is accepted only when the remote address equals this device's own
     reflexive address, which means the same NAT, so the same network (hairpinning).
   - The selected-pair check in §5.4 allows exactly that case.
   - The STUN server is preferably run with the backend, so no third party sees the addresses. S0 tests a public one
     and records which work.
4. R1–R8 land together with the web (pre-pol). §4–§7 go into `B/docs/協議.md`.
5. Accepted: union healing restores whatever any device still holds under a non-erased import, so after an erase
   every device converges to the older import's values (§6.2). On one device alone those rows stay deleted.
6. The DB design §5.3 phone line was wrong and is corrected there: every device holds the complete data.

## 15. Plan and tests

| Step | Content | Tests |
|---|---|---|
| S0 | Spike: §13 with a throwaway page, a str0m binary and a local stand-in relay | manual, results recorded here and in `perf.md` |
| S1 | core: inventory, fingerprints, `export_subset`, R1–R6, settings, sync state | property test with three profiles, synced in random pairings and orders; then compare **table contents** (`v_observation`, genotypes, corrections, sources, `import.blob`, blob files), not digests, and check every session ends. Random steps: imports, overlapping imports, edits then removes, erases, review confirm/discard (also confirmed differently on two devices), corrections, priority changes, duplicate files, NULL `end_ms`/`detail`, 5 vs 5.0. Parity fixtures for R1–R8 with the web |
| S2 | sync engine in `health-api` over an in-memory transport | interrupted batches, split imports, negative cache, confirmations, limits |
| S3 | transport: signalling client, str0m, ICE checks in `health-connect`; a stand-in relay in tests, as in `health/tests/account.rs` | two serve processes on loopback reach "up to date" |
| S4 | serve loop, `health sync status`, UI | Playwright: status rows, the first-sync confirmation |
| S5 | the web side (W) | cross-implementation: the web merges Rust-made subset packs (as `web-import-check`); shared inventory and fingerprint vectors |
| S6 | the endpoint on the dev backend, a live test on a real network | manual |

Each step lands as its own commit, with its decisions recorded in `docs/decisions.md`.
