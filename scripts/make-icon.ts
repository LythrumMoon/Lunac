// scripts/make-icon.ts
// Converts a source PNG to multi-resolution .ico for the app icon.
// Requires sharp (installed in core/).
// Usage: bun run --cwd core ../scripts/make-icon.ts <source.png> [output.ico]
//
// Or run from project root with:
//   cd core && bun run ../scripts/make-icon.ts <src> [out]

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const SIZES = [256, 128, 64, 48, 32, 16];

async function main() {
  // Resolve sharp from the script's home in core/node_modules
  const sharp = (await import("../core/node_modules/sharp/lib/index.js")).default;

  const args = process.argv.slice(2);
  if (!args[0]) {
    console.error("用法: bun run scripts/make-icon.ts <source.png> [output.ico]");
    process.exit(1);
  }
  const srcPath = args[0];
  // 默认输出到仓库内的图标位置（按脚本自身位置解析，不依赖当前工作目录）
  const outPath = args[1] || fileURLToPath(new URL("../app/src-tauri/icons/icon.ico", import.meta.url));

  const srcBuffer = readFileSync(srcPath);

  const pngEntries: { size: number; data: Buffer }[] = [];
  for (const size of SIZES) {
    const resized = await sharp(srcBuffer)
      .resize(size, size, { fit: "cover", position: "center" })
      .png()
      .toBuffer();
    pngEntries.push({ size, data: Buffer.from(resized) });
    console.log(`  ${size}x${size}  ${resized.length} bytes`);
  }

  // ICO format: 6-byte header + N * 16-byte dir entries + image data
  const headerSize = 6;
  const dirEntrySize = 16;
  let imageOffset = headerSize + dirEntrySize * pngEntries.length;

  const parts: Buffer[] = [];

  // Header
  const header = Buffer.alloc(headerSize);
  header.writeUInt16LE(0, 0);   // reserved
  header.writeUInt16LE(1, 2);   // type: 1 = icon
  header.writeUInt16LE(pngEntries.length, 4);
  parts.push(header);

  // Dir entries + image data
  for (const { size, data } of pngEntries) {
    const w = size >= 256 ? 0 : size;
    const h = size >= 256 ? 0 : size;
    const entry = Buffer.alloc(dirEntrySize);
    entry.writeUInt8(w, 0);
    entry.writeUInt8(h, 1);
    entry.writeUInt8(0, 2);       // color palette size (0 = truecolor)
    entry.writeUInt8(0, 3);       // reserved
    entry.writeUInt16LE(1, 4);    // planes
    entry.writeUInt16LE(32, 6);   // bits-per-pixel
    entry.writeUInt32LE(data.length, 8);
    entry.writeUInt32LE(imageOffset, 12);
    parts.push(entry);
    parts.push(data);
    imageOffset += data.length;
  }

  const ico = Buffer.concat(parts);
  writeFileSync(outPath, ico);
  console.log(`\nWrote ${outPath} (${ico.length} bytes, ${pngEntries.length} sizes)`);
}

main().catch((err) => {
  console.error("Icon generation failed:", err);
  process.exit(1);
});
