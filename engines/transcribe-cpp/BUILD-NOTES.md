# transcribe.cpp — P2 real-path build notes

Status updated 2026-09-21 for the 0.2.3 runtime refresh. This file records both retained
runtime paths. `runtime` is the P2 CLI-sidecar rollback path;
`resident-worker` is the optional process-isolated, model-resident CPU path.

## License review: PASSED (code)

- Upstream code license: **MIT** (`LICENSE` in the pinned clone; GitHub API
  reports `spdx_id: MIT` for handy-computer/transcribe.cpp). Verified during
  the P2 real-path build, 2026-08-22.
- The P2 `runtime` feature still links nothing upstream into Narrivy. Recovery
  4B's separate `resident-worker` feature exact-pins the published safe Rust
wrapper and its sys crate at 0.2.3. Native unsafe remains inside the upstream
  sys/wrapper dependency and a separate worker executable, never Narrivy's
  portable/core/daemon code.

## Recovery 4B decision: process-isolated resident worker

The strict optimized lane keeps one CPU model resident in
`narrivy-transcribe-worker` and sends bounded 16 kHz mono f32 PCM over private
stdin/stdout frames. Protocol v5's bounded startup frame carries the loaded
model's language list, translation targets, language-detection, translation,
streaming, and initial-prompt attestations. One-shot decode frames carry a
bounded spoken-language/translation/context request before PCM; the worker
validates it against those attestations and maps it to the pinned wrapper's
`RunOptions`. Written language, script, and locale stay absent because this
backend cannot apply them. Unknown bits, malformed or oversized payloads, and
a capability change across worker restart terminate/reject the worker. The
parent encodes PCM into a fixed 16 KiB transfer buffer
and the worker decodes fixed 16 KiB reads directly into its final f32 buffer;
neither side creates a second utterance-sized byte vector. This removes
per-utterance process/model loading and redundant payload-sized allocations
without making an unkillable native call part of the daemon. The daemon parent
owns the child and pipes, admits one request at a time, fails concurrent
requests as `asr_busy`, and kill/reaps on cancellation, timeout, protocol, or
I/O failure. The next request may create a fresh worker; there is no mock or
sidecar fallback.

For models that attest streaming support, protocol v5 additionally provides an
exclusive `begin -> feed* -> finalize|reset` lifecycle. Each feed is a bounded,
caller-paced request with no background queue. Responses carry bounded native
revision/update metadata plus full/committed/tentative UTF-8 snapshots. The
parent rejects identity, finality, revision, reserved-bit, length, and UTF-8
violations; it emits changed non-empty display partials only and uses native
full text for the final result. The caller's token covers replacement startup,
stream begin, and blocked feed/finalize. Native `STREAM_BEGIN` has a two-second
bound; replacement startup retains its separate cancellable 30-second bound.
Every uncertain begin removes and kills/reaps the worker before return. Drop
attempts a bounded reset and terminates on uncertainty.

The worker is selected only by the explicit
`NARRIVY_TRANSCRIBE_RUNTIME=resident` configuration and requires
`NARRIVY_TRANSCRIBE_WORKER`, `NARRIVY_TRANSCRIBE_WORKER_SHA256`, and
`NARRIVY_TRANSCRIBE_WORKER_SIZE` to bind one current-user-owned, safely
permissioned native ELF build artifact. Startup copies it into an executable
sealed memfd after exact size/digest validation, and every recovery generation
reuses that immutable handle without reopening the source pathname. This is
controlled-build binding, not installed-package provenance. The worker loads
the explicit model only from an inherited sealed descriptor. Resident startup
also requires `NARRIVY_TRANSCRIBE_MODEL_SHA256` and
`NARRIVY_TRANSCRIBE_MODEL_SIZE`; it copies the source once into a mode-0400
sealed memfd after safe-identity, exact-size and digest validation. Every worker
generation receives the retained descriptor through the otherwise close-on-exec
boundary and independently checks its descriptor path, seals, mode, size and
digest before native load. The original pathname is never a worker input and is
not reopened by status, admission or recovery. The worker uses `Backend::Cpu`
unless the parent asks for `auto` (see "GPU acceleration" below),
rejects backend/sample-rate/cancellation
capability mismatch, disables native logging, and intentionally creates no
audio/transcript file. OS core-dump/swap containment is not yet configured and
remains a release privacy blocker. `sidecar` remains the explicit rollback
value.

## GPU acceleration (ADR-0076)

The `vulkan` feature builds the ggml Vulkan backend into the worker
(`cargo build -p narrivy-asr-transcribe --features vulkan --bin
narrivy-transcribe-worker`). Build needs: the Vulkan headers
(`libvulkan-dev`) and `glslc`. Run needs: `libvulkan.so.1` (`libvulkan1`)
and, to use a GPU, a Vulkan driver (`mesa-vulkan-drivers`).

The parent passes `--compute auto`, `--compute gpu` (a GPU or a backend
failure, no fallback) or `--compute cpu` before the model binding arguments; without the argument the worker is strict CPU, as it
always was. For `auto`, a Vulkan build loads the model on the GPU and
decodes `tests/fixtures/jfk16.wav` (compiled into the worker) there. It
keeps the GPU only when the text contains the known phrase and the decode
was faster than real time; otherwise it releases the GPU copy and loads
strict CPU. The second byte of the ready frame's capability payload, always
zero before, attests the device: 0 CPU, 1 integrated GPU, 2 discrete GPU.
The wire version is unchanged because a worker started without `--compute`
still sends zero.

`NARRIVY_TRANSCRIBE_COMPUTE=auto|cpu` (default `auto`) sets the request. A
worker that was allowed a GPU and fails withdraws the GPU for the life of
the engine; see ADR-0076 for the replacement rules.

`examples/backend_probe.rs` lists the devices a build sees and times
one-shot and streaming decodes on a chosen backend. It prints timings and
device facts only.

Dependency pins:

- `transcribe-cpp = 0.4.1`, crates.io checksum
  `8bab65d30f5a4c3ea0ab6f38236823389f3914b0605bbc0073a4ec88018c0714`;
- `transcribe-cpp-sys = 0.4.1`, crates.io checksum
  `23a837974fdb873998a59a21ad2bf5a4b7bbf721c20d03d2a3ab8c221726a444`;
- both MIT, default features disabled, static CPU build; `Cargo.lock` is
  authoritative for transitive pins.

Version 0.4.1 (2026-10-26, source commit
`4d7c1ccefd39e5a9ea36cca5f8fefc6dcc14b99b`, tag v0.4.1) replaces 0.2.3. API
changes taken: `Model::capabilities()` now returns a `Result` (a failure is
reported as a native startup failure), and `ModelOptions` gained `no_mmap`;
the worker keeps the default (`false`): the weights are served from the
hash-checked model file's page cache instead of copied, so the system can
reclaim them. Also new upstream since 0.2.3: a repetition-loop guard,
decode-budget fix, VAD and language id in the interface, ggml 0.26, and a
Windows build fix.

Version 0.2.1 was evaluated and rejected: its published sys source reported
native version 0.2.2, so the safe wrapper's pre-1.0 ABI gate correctly refused
model load. Narrivy did not bypass the gate and moved to the matching published
pair. Version 0.2.3 supersedes that pair and retains the public C surface while
fixing buffered Parakeet end-of-stream tail loss, releasing scheduler/compute
scratch memory after a run, and adding an optional prebuilt-install link path.
Narrivy continues to build the exact locked source with default features off.

## Windows (D36, SQ2, D25): resident worker with Vulkan

The resident worker and parent build natively with MSVC (`x86_64-pc-windows-msvc`)
and the Visual Studio CMake. The pinned sys crate needs no patch: its build
script already forces `/O2` on MSVC, and ggml detects AVX2/FMA/F16C. The
release worker is built with Vulkan, as on Linux: it needs the LunarG Vulkan
SDK (headers, `glslc`, `vulkan-1.lib`; 1.4.363.0 used on 2026-10-27) and links
the system loader `vulkan-1.dll`, which every current GPU driver installs. The
sys crate asks for `vulkan-1.lib` without its folder, so the SDK's `Lib`
folder goes on the linker path.

```powershell
$env:CMAKE = "<VS>\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe"
$env:VULKAN_SDK = "C:\VulkanSDK\<version>"; $env:PATH = "$env:VULKAN_SDK\Bin;$env:PATH"
$env:RUSTFLAGS = "-L native=$env:VULKAN_SDK\Lib"
cargo build --release -p narrivy-asr-transcribe --features vulkan --bin narrivy-transcribe-worker
```

C runtime: the default build links the dynamic CRT (`/MD`, needs the VC++
runtime). A self-contained worker needs both `RUSTFLAGS=-C target-feature=+crt-static`
and `TRANSCRIBE_CMAKE_ARGS=-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`: CMake's
runtime policy otherwise keeps `/MD` for the native objects and the link fails
with `MSVCRT` conflicts. Switching an existing target directory between the two
needs a clean `transcribe-cpp-sys` build directory, or stale `/MD` objects fail
the link with `LNK2038 RuntimeLibrary` mismatches. Build the static worker with an
explicit `--target x86_64-pc-windows-msvc`: without it `RUSTFLAGS` also
applies to build scripts, and on 2026-10-21 Microsoft Defender quarantined
one such static-CRT build-script binary (`zmij`) as
`Trojan:Win64/Disdroth.EM!MTB` (a machine-learning heuristic; the same build
script built normally was not flagged). `scripts/dev-daemon.ps1` does this. Verified 2026-10-21: the
static worker (3.47 MB) imports neither `vcruntime140.dll` nor the
`api-ms-win-crt-*` sets; the default one (3.19 MB) imports both.

Measured 2026-10-21 in a Windows 11 VM (KVM, 4 vCPUs of an Intel Core Ultra 9
288V, no GPU) on `tests/fixtures/jfk16.wav` (11.0 s) with
`whisper-tiny.en-Q8_0.gguf`, through `examples/resident_transcribe.rs`:
engine startup (snapshots, spawn, model load) 436 ms (dynamic CRT) / 494 ms
(static); decode 827-864 ms (RTF 0.075-0.079) / 583-815 ms (RTF 0.053-0.074);
the known phrase was recognised on every run. A first run while the host was
oversubscribed took 22-26 s per decode, so VM timings are only meaningful on an
idle host.

What differs from Linux (same code paths otherwise, through
`narrivy-process-windows`, which has the same API as `narrivy-process-linux`):

- Worker binding: a digest-checked PE image (`MZ`/`PE`, host machine type,
  not a DLL) copied into a private snapshot directory and held open with a
  read-only share mode, so nobody can change it while the engine lives.
- Model binding: the same deny-write snapshot, passed to the worker as an
  absolute path instead of an inherited sealed memfd. The worker reopens it
  with a read-only share mode (the open fails while anyone can write it, and
  no writer can open it afterwards), checks size and SHA-256, and holds that
  handle through native load.
- Containment: the worker starts suspended inside a kill-on-close Job Object
  with a cleared environment and the system directory as its working
  directory, instead of a process group with `PR_SET_PDEATHSIG`.
- Pipes: Windows pipes have no `O_NONBLOCK`. Stdin is set to `PIPE_NOWAIT`
  (a full pipe is `WouldBlock`) and stdout is read only as far as
  `PeekNamedPipe` reports bytes waiting, so every read and write still
  honours the deadline and cancellation.
- Crash dumps: the worker opts out of Windows Error Reporting
  (`SEM_NOGPFAULTERRORBOX`). A machine-wide WER `LocalDumps` policy is outside
  its control; together with swap/pagefile containment this stays a release
  privacy blocker, as on Linux.
- Not yet on Windows: the engine's unit tests (they drive shell-script
  workers), trust-anchored (installed-package) bindings, which fail closed,
  and a non-ASCII temp path for the native loader (untested).

`examples/resident_transcribe.rs` starts the engine exactly as the daemon does
and times decodes of a WAV, printing timings and whether the fixture's known
phrase was recognised, never the transcript.

## Integration decision: versioned CLI sidecar (not staticlib)

Upstream v0.2.1 exposes two integration surfaces:

1. `include/transcribe.h` — a ~2600-line public C API plus official language
   bindings (including a Rust crate we cannot adopt without editing the root
   workspace manifest).
2. `build/bin/transcribe-cli` — a documented, stable CLI:
   `transcribe-cli -m model.gguf [-q] [--backend cpu] -o out.txt in.wav`
   (16 kHz mono WAV in; UTF-8 transcript to `-o` file or stdout).

CHOSEN: the CLI sidecar. Reasons: stable observable contract (argv in,
transcript file out, exit status), zero `unsafe`, zero new dependencies, no
root-manifest edits, trivially pinnable (binary sha256 recorded per run), and
the CPU correctness lane is selected explicitly with `--backend cpu`.

NOT chosen: linking a staticlib through a local `transcribe-sys` stub. Its
hand-written `tcpp_*` declarations were pre-licence hypotheses that did not
match the real upstream API; the stub was deleted (F-L1, 2026-10-20). The
resident worker links the upstream safe Rust wrapper (`transcribe-cpp`)
instead.

## Operator contract (env vars)

| Variable | Meaning |
| --- | --- |
| `TRANSCRIBE_CPP_BIN` | Path to the built `transcribe-cli`. REQUIRED whenever the engine loads a model in the `runtime` world. Validated (regular file) at load time; nothing is downloaded or built by Rust code. |
| model path | Constructor argument (`TranscribeCppEngine::with_model_path`). The strict Recovery 2A daemon requires an explicit `NARRIVY_TRANSCRIBE_MODEL`; it never selects a model implicitly. |

Decode argv (fixed): `<bin> -m <model> -q --backend cpu -o <out.txt> <in.wav>`
with a hard 120 s kill bound (`sidecar::DECODE_TIMEOUT`) and a content-free
temp workdir per invocation. Stdout and stderr are discarded; typed errors
contain stable reason codes only. A shared cancellation signal kills and reaps
the child within `sidecar::CANCEL_TERMINATION_BOUND` (2 seconds).

## Model pin (P2 reference)

- File: `~/.cache/narrivy/models/whisper-tiny.en-Q8_0.gguf`
- Variant: Whisper tiny.en (English-only, 39M params), quant Q8_0
- Size: 45,904,544 bytes (43.8 MiB)
- sha256: `e8c9b73c06344307d8b346e07fbe93dd88d894627854bcff31523f1ce44394fa`
- Weights license: Apache-2.0 (upstream card docs/models/whisper-tiny.en.md;
  port of openai/whisper-tiny.en @ 87c7102, pinned 2026-04-25)
- Upstream-published WER (LibriSpeech test-clean, Q8_0): 5.72%
- Source: https://huggingface.co/handy-computer/whisper-tiny.en-gguf at
  revision `becb8bcb804405dc97b380a523d9975888820986`; the published Git-LFS
  pointer carries the SHA-256 above. Recovery 2A uses the normal user cache,
  never the repository.

## Upstream pin

- Repo clone: `code/vendor/transcribe.cpp` (gitignored area, operator-built)
- Release tag: **v0.2.3**
- Commit: `63a44d9239d610b3908e8a66b384924cd4a77217`
- sha256 of `git archive --format=tar v0.2.3`:
  `aadf2b927c7c738ab47770bc2c7b45854558987c6929a52e588037cf2349e305`
- Build: `cmake -B build -DCMAKE_BUILD_TYPE=Release && cmake --build build
  --parallel 8` (CPU backend only; no Vulkan/CUDA/Metal options enabled).
  Build log tail kept at `/tmp/tcpp-cmake.log` for the session that produced
  it; rebuilds are reproducible from the pinned commit.
- Sidecar binary rebuilt and inspected on 2026-09-21:
  `code/vendor/transcribe.cpp/build-v0.2.3/bin/transcribe-cli`, size 4,347,712
  bytes, sha256
  `3285781ac28c50f27bcb337ac9afb4d6552a6f7fde3db87445eb387bdf48168e`.
  A CPU decode of the pinned JFK fixture completed successfully with the known
  phrase. Rebuilds may differ byte-wise; this development check is not release
  evidence and each native qualification records its actual hash.

## Known limitations (tracked, honest)

1. The CLI-sidecar and current Whisper tiny resident model are final-only. The
   resident worker/engine bind upstream streaming only for a model whose
   loaded capabilities attest support. A strict controlled lane now composes
   bounded live capture plus the replaceable energy-VAD/endpointer candidate,
   but native acoustic quality, threshold calibration, latency, daemon/client
   partial events, and desktop partial UX remain unqualified or uncomposed.
2. The sidecar starts one bounded decode per final audio
   buffer. It receives the transaction-wide cancellation signal and is killed
   within the two-second termination bound when cancellation is observed.
3. `Send/Sync` sharing assumptions of the old FFI sketch are gone with it;
   each stream spawns its own short-lived sidecar process instead.
4. GPU/iGPU lanes are NOT exercised at P2; `--backend cpu` is hard-coded for
   the CPU-correctness contract (P2 gate check "CPU correctness path works
   even on accelerator hosts").

## Recovery Phase 2A qualification

The binary was started successfully with `--help` on the current host. Native
model compatibility and microphone-to-Inbox behavior are established only by
a bounded real-microphone run; fixture audio and unit tests are
controlled checks, never native qualification or release evidence.

## Patched native library and shader compiler (2026-11-03)

The worker builds `transcribe-cpp-sys` from `vendor/transcribe-cpp-sys`, a
copy of the published 0.4.1 crate with an encoder-attention patch: on a GPU,
the first window is encoded with both attention modes and the faster is kept
(on an Intel Arc 140V, plain F16 attention instead of ggml-vulkan's scalar
flash attention). The worker's GPU self-test therefore runs its recording
twice and times the second run. Changes and update steps:
`vendor/transcribe-cpp-sys/NARRIVY-PATCH.md`.

`scripts/build_release_candidate.sh` requires a glslc with
`GL_EXT_integer_dot_product`, so ggml-vulkan builds its integer dot-product
shaders. Ubuntu 24.04's shaderc 2023.8 lacks it; the script then uses
`$VULKAN_SDK/bin/glslc` or `scripts/glslc-flatpak-sdk` (an installed Flatpak
`org.freedesktop.Sdk` 25.08 or later).

Measured with whisper-large-v3-turbo Q8_0 on the Arc 140V: a 30-second window
went from about 1.6 s to 0.9-1.0 s and 87 s of audio from about 5.3 s to 3.4 s.
The transcripts were byte-identical on 20 recordings.
