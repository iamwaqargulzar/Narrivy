// No device, model, runtime, delivery method or application name may be
// hard-coded in application source. The Compute tab once shipped such literals
// ('Intel NPU', 'NVIDIA RTX 3090', 'Whisper Large-v3', 'OpenVINO', 'uinput /
// ydotool', 'Portal / libei', 'X11 / XWayland', 'VS Code', 'Firefox',
// 'Discord', …) that the daemon never advertised, next to a second resolved
// compute claim that contradicted lib/resolved.ts. The guard scans src/ the way
// CapabilityStatesSingleSource.test.ts scans source text for catalog drift.
// The test directory is skipped on purpose: enumerating the forbidden literals
// is this test's job, and a fixture that needs one must fail here.
import { describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import * as path from 'node:path';

const SRC = path.resolve(process.cwd(), 'src');
const TESTS_DIR = path.join(SRC, '__tests__');
const SETTINGS_COMPUTE = path.join(SRC, 'app', 'components', 'SettingsCompute');

/** Files whose literals are the frontend's honest source of resolved values. */
const SHARED_RESOLVERS = [
  path.join(SRC, 'lib', 'resolved.ts'),
  path.join(SRC, 'lib', 'settings.ts'),
  path.join(SRC, 'app', 'screens', 'Home.tsx'),
];

/** Engine names that survive in src/ outside this tab, per the audit. */
const ENGINE_NAMES = /Whisper|OpenVINO|openvino/gi;
// The showcase fixture stands in for the daemon (README screenshots, dev
// only), so it names the models the daemon would list.
const REVIEWED_ENGINE_NAME_FILES = ['app/screens/Models.tsx', 'test/showcaseBridge.ts'];

const FABRICATED_LITERALS: ReadonlyArray<readonly [string, RegExp]> = [
  ['accelerator vendors and models', /NVIDIA|Intel\s+NPU|Intel\s+Arc|Intel\s+iGPU|AMD\s+Ryzen|Radeon/gi],
  ['uinput / ydotool', /uinput|ydotool/gi],
  ['Portal / libei', /libei|Portal\s*\/\s*libei/g],
  ['X11 / XWayland', /XWayland/g],
  ['Wayland virtual keyboard', /Wayland\s+Virtual\s+Keyboard/gi],
  [
    'invented delivery methods',
    /Direct\s*\/\s*Semantic\s+Insert|Terminal\s+Paste|Clipboard\s+Only|Narrivy\s+Inbox\s+Only|Narrivy Inbox/g,
  ],
  ['invented applications', /VS\s*Code|Firefox|Discord|Obsidian/gi],
  [
    'invented device priority ladders',
    /DEFAULT_AUTO_PRIORITY|DEFAULT_MANUAL_FALLBACK|DEFAULT_AVAILABLE_METHODS|MANUAL_DEVICES|APP_OVERRIDE_OPTIONS/g,
  ],
];

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (full === TESTS_DIR) continue;
      out.push(...sourceFiles(full));
    } else if (/\.tsx?$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

function matches(literal: RegExp, file: string): string[] {
  const text = fs.readFileSync(file, 'utf-8');
  const found: string[] = [];
  literal.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = literal.exec(text)) !== null) {
    const line = text.slice(0, match.index).split('\n').length;
    found.push(`${path.relative(SRC, file)}:${line} ${JSON.stringify(match[0])}`);
    if (match.index === literal.lastIndex) literal.lastIndex += 1;
  }
  return found;
}

function offenders(literal: RegExp, files = sourceFiles(SRC)): string[] {
  return files.flatMap((file) => matches(literal, file));
}

describe('NoFabricatedHardwareLiterals', () => {
  it('scans a non-empty part of the source tree', () => {
    const files = sourceFiles(SRC);
    expect(files.length).toBeGreaterThan(10);
    expect(files).toContain(path.join(SETTINGS_COMPUTE, 'ComputeCard.tsx'));
    expect(files).toContain(path.join(SRC, 'app', 'screens', 'Home.tsx'));
    expect(files.every((file) => !file.startsWith(TESTS_DIR))).toBe(true);
  });

  for (const [label, literal] of FABRICATED_LITERALS) {
    it(`never hard-codes ${label} anywhere in src/`, () => {
      expect(offenders(literal)).toEqual([]);
    });
  }

  it('never hard-codes an engine name in the Compute tab or the shared resolvers', () => {
    const owned = [
      ...sourceFiles(SETTINGS_COMPUTE),
      ...SHARED_RESOLVERS.filter((file) => fs.existsSync(file)),
    ];
    expect(owned).toContain(path.join(SETTINGS_COMPUTE, 'TextDeliveryCard.tsx'));
    expect(offenders(ENGINE_NAMES, owned)).toEqual([]);
  });

  it('confines any surviving engine name in src/ to the reviewed Post-1.0 list', () => {
    const filesWithEngineName = sourceFiles(SRC)
      .filter((file) => matches(ENGINE_NAMES, file).length > 0)
      .map((file) => path.relative(SRC, file));
    expect(filesWithEngineName.every((file) => REVIEWED_ENGINE_NAME_FILES.includes(file))).toBe(true);
  });

  it('never renders an emoji badge for an application in the Compute tab', () => {
    const emoji = /\p{Extended_Pictographic}/u;
    expect(offenders(emoji, sourceFiles(SETTINGS_COMPUTE))).toEqual([]);
  });
});
