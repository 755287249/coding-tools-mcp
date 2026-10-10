import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { parse } from "svelte/compiler";
import ts from "typescript";

const root = process.cwd();

async function importTypeScriptModule(file) {
  const source = await readFile(file, "utf8");
  const compiled = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2022,
    },
  }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);
}

async function sourceFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map(async (entry) => {
      const absolute = path.join(directory, entry.name);
      if (entry.isDirectory()) return sourceFiles(absolute);
      return entry.name.endsWith(".svelte") ? [absolute] : [];
    }),
  );
  return nested.flat();
}

function literalVisibleText(source) {
  const ast = parse(source, { modern: true });
  const values = [];

  function visit(value, key = "") {
    if (key === "attributes" || !value || typeof value !== "object") return;
    if (Array.isArray(value)) {
      value.forEach((item) => visit(item));
      return;
    }
    if (value.type === "Text" && value.data.trim()) {
      values.push(value.data.trim());
      return;
    }
    Object.entries(value).forEach(([childKey, child]) => visit(child, childKey));
  }

  visit(ast.fragment);
  return values.filter((value) => /\p{L}/u.test(value));
}

test("i18n exposes complete four-locale messages with an English fallback", async () => {
  const { MESSAGES } = await importTypeScriptModule(
    path.join(root, "src", "lib", "i18n", "catalog.ts"),
  );

  assert.ok(Object.keys(MESSAGES).length > 100);
  for (const [key, translations] of Object.entries(MESSAGES)) {
    assert.equal(translations.length, 4, `${key} must define en, zh-TW, zh-CN, and ja`);
    translations.forEach((translation, index) => {
      assert.equal(typeof translation, "string", `${key}[${index}] must be a string`);
      assert.ok(translation.trim(), `${key}[${index}] must not be empty`);
    });
  }

  const runtime = await readFile(path.join(root, "src", "lib", "i18n", "index.ts"), "utf8");
  assert.match(runtime, /DEFAULT_LOCALE:\s*Locale\s*=\s*"en"/);
  assert.match(runtime, /\["en",\s*"zh-TW",\s*"zh-CN",\s*"ja"\]/);
  assert.match(runtime, /loadLocalePreference/);

  const appHtml = await readFile(path.join(root, "src", "app.html"), "utf8");
  assert.match(appHtml, /<html lang="en"/);
});

test("language selector keeps native options legible in the dark sidebar", async () => {
  const component = await readFile(
    path.join(root, "src", "lib", "components", "LanguageSelect.svelte"),
    "utf8",
  );

  const css = await readFile(path.join(root, "src", "app.css"), "utf8");
  assert.match(component, /class="tx-lang-select/);
  assert.match(css, /color-scheme:\s*dark/);
  assert.match(css, /\.tx-lang-select option\s*\{[^}]*background-color:\s*var\(--surface-2\)/s);
  assert.match(css, /\.tx-lang-select option\s*\{[^}]*color:\s*var\(--text-main\)/s);
});

test("visible Svelte prose is routed through i18n without assuming a language", async () => {
  const files = await sourceFiles(path.join(root, "src"));
  const allowedTechnicalText = new Set([
    "Coding Tools",
    "Coding Tools MCP",
    "v",
    "Bearer Token",
    "· docs/history-session",
    "P95",
    "FRP",
    "Cloudflare",
    "Token:",
    "Quick Tunnel",
    "Named Tunnel",
    "MCP",
    "Streamable HTTP ·",
    "Actions",
    "OpenAPI",
    "docs/history-session",
    "Token",
    "· Token",
  ]);

  for (const file of files) {
    const source = await readFile(file, "utf8");
    const untranslated = literalVisibleText(source).filter(
      (value) => !allowedTechnicalText.has(value),
    );
    assert.deepEqual(
      untranslated,
      [],
      `${path.relative(root, file)} contains visible prose outside i18n`,
    );
  }
});

test('system locale follows ordered visitor languages and Chinese script/region variants', async () => {
  const { resolveSystemLocale } = await importTypeScriptModule(path.join(root, 'src/lib/i18n/preference.ts'));
  for (const [languages, expected] of [
    [['zh-CN'], 'zh-CN'], [['zh'], 'zh-CN'], [['zh-SG'], 'zh-CN'],
    [['zh-TW'], 'zh-TW'], [['zh-HK'], 'zh-TW'], [['zh-MO'], 'zh-TW'],
    [['zh-Hans-TW'], 'zh-CN'], [['zh-Hant-CN'], 'zh-TW'], [['ZH_hAnT_hK'], 'zh-TW'],
    [['ja-JP'], 'ja'], [['en-GB','zh-CN'], 'en'], [['fr-FR','ja-JP'], 'ja'],
    [['fr-FR'], 'en'], [[], 'en'],
  ]) assert.equal(resolveSystemLocale(languages), expected, languages.join(','));
});

test('default and legacy automatic English migrate to system while explicit preferences persist', async () => {
  const { loadLocalePreference, LOCALE_PREFERENCE_KEY } = await importTypeScriptModule(path.join(root, 'src/lib/i18n/preference.ts'));
  const read = values => key => values[key] ?? null;
  assert.equal(loadLocalePreference(read({})), 'system');
  assert.equal(loadLocalePreference(read({'coding-tools.locale':'en'})), 'system');
  assert.equal(loadLocalePreference(read({'coding-tools.locale':'zh-TW'})), 'zh-TW');
  assert.equal(loadLocalePreference(read({'coding-tools.locale':'ja'})), 'ja');
  assert.equal(loadLocalePreference(read({[LOCALE_PREFERENCE_KEY]:'en','coding-tools.locale':'zh-CN'})), 'en');
  assert.equal(loadLocalePreference(read({[LOCALE_PREFERENCE_KEY]:'system','coding-tools.locale':'ja'})), 'system');
  assert.equal(loadLocalePreference(read({[LOCALE_PREFERENCE_KEY]:'invalid','coding-tools.locale':'invalid'})), 'system');
});

test('blocked browser storage still permits the system language default', async () => {
  const { loadLocalePreference } = await importTypeScriptModule(path.join(root, 'src/lib/i18n/preference.ts'));
  assert.equal(loadLocalePreference(() => { throw new Error('Storage denied'); }), 'system');
});
