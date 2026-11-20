#!/usr/bin/env node
// 把三个上游生态的索引聚合成一份 registry/index.json。
//
// 设计边界：
//  - 只读本地 `registry/upstream/*.json` 与 `registry/sources/*.json`，不联网、不抓取站点；
//  - 上游只记录地址与元数据，不复制规则 / 脚本正文（安装时由插件按地址读取）；
//  - `registry/sources/*.json` 是**本仓库自建**来源，规则正文可内嵌进 registry 便于安装；
//  - license / attribution 原样保留，不允许抹掉。
//
// 用法：先 `npm run build`，再 `npm run registry:build`。

import { readFileSync, writeFileSync, existsSync, mkdirSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  fromMangaIndex,
  fromVideoIndex,
  fromComicIndex,
  fromCmsIndex,
  mergeRegistry,
} from "../packages/rh-source-sdk/dist/index.js";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CONFIG_PATH = join(ROOT, "registry", "config.json");

const DEFAULTS = {
  manga: {
    index: "registry/upstream/manga.json",
    baseUrl: "",
    attribution: "KazumiRules (MIT)",
  },
  comic: {
    index: "registry/upstream/comic.json",
    baseUrl: "",
    attribution: "Venera comic sources",
  },
  video: {
    index: "registry/upstream/uzvideo.json",
    baseUrl: "",
    attribution: "uzVideo extensions",
  },
  cms: {
    index: "registry/upstream/cms.json",
    baseUrl: "",
    attribution: "公开 TVBox / LibreTV 配置聚合",
  },
};

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function main() {
  const config = existsSync(CONFIG_PATH)
    ? { ...DEFAULTS, ...readJson(CONFIG_PATH) }
    : DEFAULTS;

  const entries = [];
  const report = [];

  for (const [name, fallback] of Object.entries(DEFAULTS)) {
    // config.json 里 uzVideo 一节历史上写作 `uzvideo`（见 registry/config.json）；
    // 两种键都接受，否则这份索引会被静默跳过（2026-10-21）。
    const cfg = {
      ...fallback,
      ...(config[name] ?? {}),
      ...(name === "video" ? (config.uzvideo ?? {}) : {}),
    };
    const file = join(ROOT, cfg.index);
    if (!existsSync(file)) {
      report.push(`${name}: 跳过（缺少 ${cfg.index}）`);
      continue;
    }
    const raw = readJson(file);
    let list = [];
    if (name === "manga") {
      list = fromMangaIndex(raw, cfg.baseUrl, cfg.attribution);
    } else if (name === "comic") {
      list = fromComicIndex(raw, cfg.baseUrl, cfg.attribution);
    } else if (name === "cms") {
      list = fromCmsIndex(raw, cfg.attribution);
    } else {
      list = fromVideoIndex(raw, cfg.baseUrl, cfg.attribution);
    }
    entries.push(...list);
    report.push(`${name}: ${list.length} 条`);
  }

  // 本仓库自建的演示来源：规则正文内嵌，导入 registry 后可直接安装（离线演示）。
  // 上游 Kazumi / Venera / uzVideo 仍只记录地址，安装时由插件读取，本仓库不保存正文。
  const sourcesDir = join(ROOT, "registry", "sources");
  let manual = 0;
  if (existsSync(sourcesDir)) {
    for (const file of readdirSync(sourcesDir).filter((name) => name.endsWith(".json")).sort()) {
      let raw;
      try {
        raw = readJson(join(sourcesDir, file));
      } catch (error) {
        report.push(`manual: 跳过 ${file}（${error instanceof Error ? error.message : String(error)}）`);
        continue;
      }
      if (!raw || typeof raw !== "object" || !raw.manifest || !raw.rule) {
        report.push(`manual: 跳过 ${file}（缺少 manifest / rule）`);
        continue;
      }
      const m = raw.manifest;
      if (!m.id || !m.name) {
        report.push(`manual: 跳过 ${file}（manifest 缺 id / name）`);
        continue;
      }
      entries.push({
        id: String(m.id),
        name: String(m.name),
        upstream: "manual",
        upstreamId: String(m.id),
        version: String(m.version ?? "0.0.0"),
        kind: Array.isArray(m.kind) ? m.kind : ["novel"],
        engine: String(m.engine ?? "declarative"),
        homepage: m.homepage,
        license: m.license,
        attribution: m.attribution,
        warnings: ["本仓库自建演示来源；规则内嵌，可直接安装"],
        source: raw,
      });
      manual += 1;
    }
  }
  report.push(`manual: ${manual} 条`);

  const bundle = mergeRegistry(entries);
  const out = join(ROOT, "registry", "index.json");
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, `${JSON.stringify(bundle, null, 2)}\n`, "utf8");

  for (const line of report) console.log(`[registry] ${line}`);
  console.log(`[registry] 合并后 ${bundle.entries.length} 条 → ${out}`);
}

main();
