import { describe, expect, it } from "vitest";

import { InstallOverall } from "./download-progress";

/** 宿主 `plugin-install-progress` 的一条上报（字段与 `DependencyProgress` 同形）。 */
function ev(
  phase: string,
  downloaded: number,
  total: number,
  index: number,
  count: number,
) {
  return { phase, downloaded, total, index, count };
}

/**
 * 2026-10-27 用户报「桌宠下载时进度条不断抖动」：宿主逐个文件下载，
 * 每条上报的 `downloaded/total` 只针对当前文件 ⇒ 换文件时环会 100% → 0%。
 * 这里钉住「整次安装的总进度」只增不减。
 */
describe("InstallOverall", () => {
  it("插件包本体那一段不给数字（宁可不画，也不先 100% 再掉回 0%）", () => {
    const o = new InstallOverall();
    expect(o.push(ev("download", 0, 400_000, 0, 0)).percent).toBe(-1);
    expect(o.push(ev("download", 200_000, 400_000, 0, 0)).percent).toBe(-1);
    expect(o.push(ev("download", 400_000, 400_000, 0, 0)).percent).toBe(-1);
  });

  it("换文件时只增不减（桌宠形状：本体 + 2 条依赖）", () => {
    const o = new InstallOverall();
    const seen: number[] = [];
    const feed = (p: ReturnType<typeof ev>) => {
      const s = o.push(p, 1_000);
      if (s.percent >= 0) seen.push(s.percent);
    };

    feed(ev("download", 400_000, 400_000, 0, 0)); // 插件包本体
    feed(ev("download", 0, 50_000, 1, 2)); // Live2D Core 起步 —— 不能是 0
    feed(ev("download", 25_000, 50_000, 1, 2));
    feed(ev("download", 50_000, 50_000, 1, 2));
    feed(ev("download", 0, 149_000_000, 2, 2)); // ZipVoice 起步 —— 不能回落到 0
    feed(ev("download", 74_500_000, 149_000_000, 2, 2));
    feed(ev("download", 149_000_000, 149_000_000, 2, 2));

    expect(seen.length).toBeGreaterThan(0);
    for (let i = 1; i < seen.length; i++) {
      expect(seen[i]).toBeGreaterThanOrEqual(seen[i - 1]);
    }
    // 三步等权：本体 1/3、Core 2/3、ZipVoice 3/3
    expect(seen[0]).toBe(33);
    expect(seen[2]).toBe(66);
    expect(seen[seen.length - 1]).toBe(100);
  });

  it("只补依赖（没有插件包本体那一步）时步数按依赖条数算", () => {
    const o = new InstallOverall();
    expect(o.push(ev("download", 0, 2_000, 1, 2)).percent).toBe(0);
    expect(o.push(ev("download", 1_000, 2_000, 1, 2)).percent).toBe(25);
    expect(o.push(ev("download", 2_000, 2_000, 1, 2)).percent).toBe(50);
    expect(o.push(ev("download", 0, 8_000, 2, 2)).percent).toBe(50);
    expect(o.push(ev("download", 8_000, 8_000, 2, 2)).percent).toBe(100);
  });

  it("解压阶段不把环退回 0（停在这一步已完成的刻度上）", () => {
    const o = new InstallOverall();
    o.push(ev("download", 2_000, 2_000, 1, 2));
    const beforeExtract = o.push(ev("download", 2_000, 2_000, 1, 2)).percent;
    const duringExtract = o.push(ev("extract", 0, 0, 1, 2));
    expect(duringExtract.extracting).toBe(true);
    expect(duringExtract.percent).toBe(beforeExtract);
    expect(duringExtract.percent).toBeGreaterThanOrEqual(0);
  });

  it("已下字节跨文件累加（「共下多少」的文案不会每次归零）", () => {
    const o = new InstallOverall();
    expect(o.push(ev("download", 400_000, 400_000, 0, 0)).downloaded).toBe(400_000);
    expect(o.push(ev("download", 0, 50_000, 1, 2)).downloaded).toBe(400_000);
    expect(o.push(ev("download", 10_000, 50_000, 1, 2)).downloaded).toBe(410_000);
  });
});
