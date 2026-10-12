# ORBIT — PC Workbench

ORBIT is a local-first Windows troubleshooting workbench. It provides read-only system inventory, bounded network diagnostics, local diagnostic history, a user-directed HTML report export, deterministic evidence interpretation, Universal Search, an optional local Ollama explanation, and a local theme preference. ORBIT does not modify Windows configuration or send telemetry.

## Requirements

- Windows 10 or 11 (x64)
- Node.js and npm
- Rust stable with the `x86_64-pc-windows-msvc` target
- Microsoft C++ Build Tools (MSVC) and Windows SDK
- Microsoft Edge WebView2 Runtime (normally present on Windows 10/11)

Install Node.js from [nodejs.org](https://nodejs.org/) and Rust using [rustup](https://rustup.rs/). Add the Rust target with `rustup target add x86_64-pc-windows-msvc`. Install the Visual Studio **Desktop development with C++** workload using the [Visual Studio Installer](https://visualstudio.microsoft.com/downloads/). Install the [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) if it is absent. ORBIT does not install system prerequisites automatically.

## Setup and development

```powershell
git clone https://github.com/Cartiace24/Orbit.git
cd Orbit
npm ci
npm run tauri:dev
```

If you already have the source, run `npm ci` in its project directory. For frontend-only iteration, use `npm run dev`. Native Tauri commands require the desktop app.

## Checks and builds

```powershell
npm run build                 # Strict TypeScript project checks + production Vite build
cargo fmt --manifest-path src-tauri\Cargo.toml -- --check
cargo check --manifest-path src-tauri\Cargo.toml
cargo test --manifest-path src-tauri\Cargo.toml
npm run tauri:build           # Windows installer bundles (MSI and NSIS)
```

MSI packaging may require the WiX Toolset; NSIS packaging may require NSIS. To compile the desktop executable without installer bundles, run `npm run tauri -- build --no-bundle`.

## Architecture

- `src/` contains the React + strict TypeScript presentation, navigation, typed native command response, diagnostic pages, and local theme preference.
- `src-tauri/src/lib.rs` contains the existing read-only Windows inventory command and app setup.
- `src-tauri/src/diagnostics.rs` validates hostnames and implements bounded DNS, TCP connectivity/latency, and IPv4 ICMP gateway probes.
- `src-tauri/src/history.rs` owns parameterized SQLite operations and the HTML report renderer. `src-tauri/migrations/` contains versioned schema SQL; SQLite `user_version` advances to version 2, which adds nullable per-result completion timestamps. New results store their actual completion time; legacy rows remain explicitly unavailable because their per-test times were not recorded.
- `src-tauri/src/intelligence.rs` reads the existing live inventory command and the same recent-session query from `history.rs`. Its deterministic rules produce explicit observation/evidence references separately from possible explanations; it does not store derived findings or create another database.
- `src-tauri/src/local_ai.rs` implements the optional local provider boundary. It calls only fixed `127.0.0.1:11434` Ollama endpoints, assembles a bounded evidence summary, and validates structured model output against real evidence IDs and supported read-only action IDs. It does not execute model suggestions.
- `src-tauri/src/search.rs` matches bounded natural-language queries against fixed ORBIT features and read-only actions, the existing SQLite history reader, and deterministic rule findings. It does not create another index or database and never runs a diagnostic or calls Ollama.
- `src/SearchDialog.tsx` and `src/search.types.ts` provide the keyboard search dialog and typed native response. Search results route through the existing navigation, History focus, and Intelligence action preview.
- `src/IntelligencePage.tsx` implements a fixed local intent parser experience, finding cards, history links, and explicit previews before invoking the existing read-only diagnostic command. `src/intelligence.types.ts` contains the typed response and action boundary.
- `src-tauri/capabilities/default.json` grants only the dialog plugin's save permission for explicit HTML exports. The application has no shell or general filesystem plugin access.

Diagnostic history is stored in the application's local data directory as `diagnostics.sqlite3`. Completed test details and sanitized application error descriptions are retained on the device. History is limited to the latest 250 sessions in the UI. Export opens a native Save dialog and writes a self-contained HTML report only to the destination selected by the user.

## Universal Search

Open Search ORBIT from the top bar or press **Ctrl+K**. Search for an ORBIT page, a network symptom, a saved diagnostic result, or a supported read-only check. Arrow keys select results, Enter opens one, and Escape closes search. Each result names its source; saved results link to their actual History session. An unavailable History query is reported while page and action matches remain available. Search reads at most the 250 sessions exposed by the existing history reader.

Search is deterministic and works without Ollama or internet access. An Intelligence result opens the question in the existing assistant for the user to submit explicitly. An action result opens the existing preview; only **Run and save test** executes a diagnostic. Search does not index unrelated Windows files, run commands, or perform general web search.

## Network diagnostic limits

- Connectivity and latency measure TCP connection setup to port 443. They do not test TLS, HTTP behavior, service health, or prove a remote host is offline.
- DNS uses the Windows-configured resolver and returns resolved addresses without exposing resolver configuration.
- Gateway discovery reads Windows adapter gateway data; reachability sends one bounded IPv4 ICMP echo. Firewall and ICMP filtering can make this check inconclusive.
- DNS has a 4-second limit, each TCP connection attempt has a 3-second limit, gateway ICMP has a 1.2-second limit, and latency uses five sequential samples.
- The probes are read-only, require no administrator privileges, and do not collect temperature, drive health, GPU telemetry, or other unsupported measurements.

There is no login, cloud service, or Supabase integration. SQLite is bundled through `rusqlite` so the app does not depend on a machine-installed SQLite library.

## Optional local AI

Local AI is off by default. In Intelligence, enable it, start Ollama yourself, refresh the installed-model list, and select a downloaded model. ORBIT never installs Ollama or downloads models. Follow [Ollama's installation and local model instructions](https://docs.ollama.com/quickstart) if needed. Model speed and memory use vary by hardware. If Ollama or a selected model is unavailable, ORBIT still runs its deterministic rules.

ORBIT uses Ollama's documented local `GET /api/tags` and `POST /api/chat` endpoints with nonstreaming JSON-schema output. The backend contacts only loopback, bypasses proxies, rejects redirects and `:cloud` model tags, and accepts only installed models that report local file size. Do not configure Ollama itself to relay local requests to a cloud model. No API key or account is used. Tauri's WebView capabilities do not grant frontend network access to Ollama; React calls narrow Rust commands.

For a question, ORBIT may send the question, selected Windows version/build/architecture, processor and memory fields, manufacturer/model, up to four recent diagnostic sessions, and up to eight deterministic findings to the local Ollama process. It omits the computer name, DNS addresses, full diagnostic details, and older database rows. The bounded context may still include tested hostnames and diagnostic outcomes. Questions and model answers are not saved to SQLite; only the enabled preference and model name are saved in WebView local storage. Disable local AI to stop sending context. Model explanations are labeled separately from verified findings. Model text is untrusted, so check the cited ORBIT result before relying on it.

## Intelligence rules

The question parser is intentionally limited to explicit topics and synonyms: DNS, gateway, latency, connectivity, network overview, system inventory availability, and repeated failures. Unsupported questions do not receive a generated diagnosis. Suggested checks show their purpose and target before the user confirms; confirmed checks invoke the existing native diagnostic command and save through the existing SQLite history path.

The Rust registry in `src-tauri/src/intelligence.rs` documents condition, required evidence, risk, explanation, recommended steps, limitations, and weakening evidence for each stable rule ID:

- `NET-DNS-001`: failed DNS plus successful gateway result in the same recorded session.
- `NET-GATEWAY-001`: recorded non-success gateway result; ICMP filtering can make reachability inconclusive.
- `NET-LATENCY-001`: at least three successful samples averaging 150 ms or more, or a mixed successful/failed set. The threshold is a screening heuristic for this TCP probe.
- `NET-TIMEOUT-001`: explicit timeout or unavailable-interface evidence.
- `SYS-INVENTORY-001`: inventory collection error or fields explicitly unavailable in the current collection.
- `NET-REPEAT-001`: three distinct sessions with the same test, normalized target, status, and sanitized error, absent a newer success.
- `INTEL-EVIDENCE-001` and `INTEL-HISTORY-001`: explicit insufficient evidence and unreadable local history states.

Repeated-failure comparisons are limited to the latest 250 saved sessions. The interface labels findings as confirmed observations, strong indications, possible causes, or insufficient evidence. The deterministic rule engine remains authoritative for findings, confidence, risk, and diagnostic outcomes.

## License

ORBIT is released under the [MIT License](LICENSE). Diagnostic history, local model files, build outputs, and machine-specific configuration are not part of the repository.
