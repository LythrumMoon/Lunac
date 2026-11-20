#!/usr/bin/env node
// 校验一份来源文件：RSP 清单 / 漫画规则 / registry bundle 三种都认。
//
// AI 生成规则后应先跑这个脚本，再把结果交给用户确认。
// 用法：node scripts/validate-source.mjs <file.json>

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  RspError,
  convertMangaRule,
  parseRegistry,
  validateSourceManifest,
} from "../packages/rh-source-sdk/dist/index.js";

const file = process.argv[2];
if (!file) {
  console.error("用法：node scripts/validate-source.mjs <file.json>");
  process.exit(2);
}

let raw;
try {
  raw = JSON.parse(readFileSync(resolve(file), "utf8"));
} catch (error) {
  console.error(`读取/解析失败：${error instanceof Error ? error.message : String(error)}`);
  process.exit(2);
}

try {
  if (raw && typeof raw === "object" && "manifest" in raw && "rule" in raw) {
    const manifest = validateSourceManifest(raw.manifest);
    if (manifest.engine !== "declarative") {
      throw new RspError("unsupported", `当前只校验 declarative 来源：${manifest.engine}`);
    }
    console.log(`OK RSP source：${manifest.id} (${manifest.kind.join("/")})`);
    process.exit(0);
  }
  if (raw && typeof raw === "object" && "rsp" in raw && "entries" in raw) {
    const bundle = parseRegistry(raw);
    console.log(`OK registry：${bundle.entries.length} 条`);
    process.exit(0);
  }
  if (raw && typeof raw === "object" && "rsp" in raw) {
    const manifest = validateSourceManifest(raw);
    console.log(`OK RSP：${manifest.id} (${manifest.kind.join("/")})`);
    process.exit(0);
  }
  const converted = convertMangaRule(raw);
  console.log(`OK 漫画规则 → RSP：${converted.manifest.id}`);
  if (converted.warnings.length) {
    for (const warning of converted.warnings) console.log(`  warning: ${warning}`);
  }
  process.exit(0);
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`${error instanceof RspError ? `[${error.code}] ` : ""}${message}`);
  process.exit(1);
}
