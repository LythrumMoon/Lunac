// ── Live2D 引擎层的「重」那一半（L1，2026-09-30）──────────────────────
//
// 只有这里 import pixi 与 `pixi-live2d-display-lipsyncpatch`。
// **它不是 `index.js` 的一部分**：由 `scripts/build-plugins.ps1` 的 pet.`extraEntries` 单打
// 成一个自包含 ESM，落在 `<Modules>\pet\engine\live2d-engine.js`；运行时由 `pet.ts` 在
// Cubism Core 就绪之后 `import(convertFileSrc(那个绝对路径))` 拉进来。
//
// **为什么必须晚于 Core**：`pixi-live2d-display` 在**模块求值时**就
// `if (!window.Live2DCubismCore) throw`（其 dist `cubism4.es.js` 末尾）。Rollup 的
// `inlineDynamicImports`（插件包必须单文件）会把「同包内惰性 import」提前到顶层求值，
// 所以只能拆成独立文件 —— 完整理由见 `./live2d.ts` 的文件头。
//
// 这一份的对外面就一个 `mountLive2D`（接口类型在 `./live2d.ts`，调用方不必被拖进 pixi）。

// ⚠️ **必须是本文件的第一条 import**（2026-10-09）：release 打包版套的是宿主的严格 CSP
// （`tauri.conf.json` 的 `script-src` **不含 `unsafe-eval`**），pixi 在这种环境里创建渲染器时
// 会抛
//   「Current environment does not allow unsafe-eval, please use @pixi/unsafe-eval module
//    to enable support」
// —— dev 的 Vite dev-server 不套那份 CSP，所以**只在 release 复现**（用户报「dev 正常、
// release 桌宠加载失败」）。`@pixi/unsafe-eval` **在导入时自安装**（7.1.0 起自 patch
// `ShaderSystem.prototype`），用无 `new Function` 的路径替换 pixi 的同步 uniform 代码生成；
// 它必须在**创建 Renderer 之前**跑完 —— 放文件头即满足（模块求值早于 `mountLive2D`）。
// 依赖必须与 `pixi.js` 同大版本（当前 7.4.3），否则 `@pixi/core` 会解析出两份实例、patch 落空。
import "@pixi/unsafe-eval";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { Application } from "pixi.js";
import { Live2DModel } from "pixi-live2d-display-lipsyncpatch/cubism4";
import {
  assertCubism4,
  dirName,
  joinPath,
  listExpressions,
  MOTION_EXTRA_GROUP,
  PET_SCALE_MAX,
  PET_SCALE_MIN,
  readModelJson,
  rewriteRefs,
  type Live2DHandle,
  type MountOptions,
} from "./live2d";

/** 建画布 + 载模型，返回一个可停可毁的句柄。失败时抛错，调用方负责把消息显示出来。 */
export async function mountLive2D(host: HTMLElement, opts: MountOptions): Promise<Live2DHandle> {
  const modelPath = opts.modelPath.trim();
  if (!modelPath) throw new Error("没有模型路径");

  const json = await readModelJson(modelPath);
  assertCubism4(json);

  // 引用**必须自己改写**：asset 协议把整条绝对路径 percent-encode 进 URL 最后一段，
  // `new URL(相对引用, convertFileSrc(模型路径))` 会落到协议根、不是模型目录
  //（见 docs/agent-feature-backlog.md 的 L1 ①）。
  const modelDir = dirName(modelPath);
  const settings = rewriteRefs(json, rel => convertFileSrc(joinPath(modelDir, rel)));

  // ── 把「引擎扫不到的表情」也接进引擎（2026-10-09；2026-10-10 加 extraExpressions）──
  // 库的 `model.expression(id)` 按 `definitions[].Name` 查表（见 `setExpression` 的注释），
  // 而**只有** `settings.FileReferences.Expressions` 里的条目才会进 `definitions`；
  // 不登记的项就是「列得出、点了切不动」（假按钮）。两个来源，都并进来：
  //   ① 模型文件**同层**、`model3.json` 未声明的 `.exp3.json`（`listExpressions` 的 extra）；
  //   ② 桌宠窗从 `lunac.hotkeys.json` 收来的 `extraExpressions`（导入到子目录 / 别处的表情）。
  // 必须在 `rewriteRefs` **之后**追加：那一份已把 `File` 统一改成 asset URL，我们照此转换；
  // 并**按 asset URL 去重**（同一条表情被两个来源都提到时只登记一次）。
  const fr = settings.FileReferences ?? {};
  const registered = new Set(
    (fr.Expressions ?? [])
      .map(e => (typeof e.File === "string" ? e.File : "").toLowerCase())
      .filter(Boolean),
  );
  const additions: { File: string; Name: string }[] = [];
  const pushExpr = (name: string, file: string) => {
    if (!name || !file) return; // 无名的登记了也切不到（`expression(Name)` 查不到）
    const url = convertFileSrc(file);
    const key = url.toLowerCase();
    if (registered.has(key)) return;
    registered.add(key);
    additions.push({ File: url, Name: name });
  };
  const extra = (await listExpressions(modelPath, json)).filter(e => !e.declared && e.file);
  for (const e of extra) pushExpr(e.name, e.file);
  for (const e of opts.extraExpressions ?? []) pushExpr(e.name, e.file);
  if (additions.length) {
    fr.Expressions = [...(fr.Expressions ?? []), ...additions];
    settings.FileReferences = fr;
  }

  // ── 把「引擎不知道的动作」接进引擎（2026-10-10）──
  // 与上面表情同一条理由：库的 `model.motion(组, 序号)` 只认 `settings.FileReferences.Motions`。
  // 用户口径是「预设动作扫文件夹里真实的 `*.motion3.json`」⇒ 这里把它们收进一个**合成组**
  // （`MOTION_EXTRA_GROUP`），**组内顺序 = 传入数组顺序 = `list_motion_files` 的路径序**，
  // 于是前端按同一份清单的下标播放就能对上。`FadeIn/OutTime` 给个温和的 0.3s（库里缺省也一样）。
  const extraMotions = (opts.extraMotions ?? []).filter(m => m && m.file);
  if (extraMotions.length) {
    const add = extraMotions.map(m => ({
      File: convertFileSrc(m.file),
      FadeInTime: 0.3,
      FadeOutTime: 0.3,
    }));
    fr.Motions = {
      ...(fr.Motions ?? {}),
      [MOTION_EXTRA_GROUP]: [...(fr.Motions?.[MOTION_EXTRA_GROUP] ?? []), ...add],
    };
    settings.FileReferences = fr;
  }

  // ── 「循环播放」动作（2026-10-10 用户口径；2026-10-10 二改；语义更正：与「待机」无关）──
  // **为什么不再借库自带的 idle 组**：库的 `MotionManager.update` 每帧在「动作播完」时会
  // `startRandomMotion(groups.idle, MotionPriority.IDLE)` —— 这条路**不可控**（随机；且库里
  // 的 motion 状态机一旦被夹在「idle 预约」与「用户显式播放」之间就会互相顶掉，实测表现是
  // **一播放动作整个渲染就永久卡死**）。改成**引擎自己驱动**（见下面 `app.ticker` 那段）：
  //   队列空了就按顺序播下一个待机动作；用各文件自己的 `Meta.Duration` 做兜底超时，
  //   某个文件「永远不结束」时强制切下一个。
  // 这里只把「文件路径 → 合成组里的下标」对上（前端传的是路径，下标按 `list_motion_files` 序）。
  const idxOfMotionFile = new Map(extraMotions.map((m, i) => [m.file, i]));
  const idleList = (opts.idleMotions ?? [])
    .map(m => ({ index: m && m.file ? idxOfMotionFile.get(m.file) ?? -1 : -1, file: m?.file ?? "" }))
    .filter(m => m.index >= 0 && m.file);

  // 库的 `ModelSettings` 构造器**强制要求** `url` 是字符串（只用于 name 推断与兜底解析），
  // 给原始 model3.json 的绝对 URL 即可 —— 引用已经全部改写过了。
  (settings as { url?: string }).url = convertFileSrc(modelPath);

  const app = new Application({
    width: Math.max(1, host.clientWidth),
    height: Math.max(1, host.clientHeight),
    backgroundAlpha: 0,
    antialias: true,
    resolution: window.devicePixelRatio || 1,
    autoDensity: true,
    // 独占 ticker：窗口隐藏时要**只停自己这一个**，不能去停 `Ticker.shared`
    sharedTicker: false,
    autoStart: true,
  });
  const canvas = app.view as HTMLCanvasElement;
  canvas.style.display = "block";
  host.appendChild(canvas);

  let model: Live2DModel;
  try {
    model = await Live2DModel.from(settings as never, {
      ticker: app.ticker,
      // 桌宠窗的左键已经归「拖窗」（`startDragging`）⇒ 指针互动本来就用不上；
      // 关掉它也省掉对 pixi 事件系统的一层依赖（模型仍会自己播 Idle 动作）。
      autoHitTest: false,
      autoFocus: false,
    });
  } catch (e) {
    // 销毁失败不能盖掉「载入失败」这个更重要的原因（同 `destroy()` 的说明）。
    try {
      app.destroy(true, { children: true, texture: true, baseTexture: true });
    } catch {
      /* 吞掉：见下方 destroy() 的注释 */
    }
    canvas.remove(); // 同上：destroy 抛错时 removeView 到不了，画布要自己摘
    throw e instanceof Error ? e : new Error(String(e));
  }
  app.stage.addChild(model as never);

  // ── 渲染循环**兜底**（2026-10-10）──────────────────────────────────
  // pixi 的 ticker 是这样重排下一帧的（`@pixi/ticker` 的 `_tick`）：
  //   `this._requestId = null; this.started && (this.update(time), …this._requestId === null && requestAnimationFrame(this._tick))`
  // ⇒ **只要 `update()` 抛一次异常，后面那句 `requestAnimationFrame` 就永远执行不到**，
  //   整个渲染**永久停住** —— 用户看到的正是「模型完全卡死」。库的 motion 状态机在某些
  //   输入下确实会抛（本次修的 idleness 冲突就是一条）。这里把内部模型的 `update` 包一层：
  //   出错的帧**吞掉并记一条日志**，绝不让它把 rAF 链掐断。
  const internalUpdate = model.internalModel.update.bind(model.internalModel);
  let updateErrs = 0;
  (model.internalModel as unknown as { update: (dt: number, now: number) => void }).update = (dt, now) => {
    try {
      internalUpdate(dt, now);
    } catch (e) {
      if (updateErrs < 5) {
        updateErrs++;
        void invoke("log_frontend", {
          level: "error",
          message: `[pet engine] internalModel.update threw: ${e instanceof Error ? e.message : String(e)}`,
        }).catch(() => {});
      }
    }
  };

  // ── 「循环播放」的驱动（2026-10-10 二改：不再依赖库的 idle 组）────────
  // 每帧看一次「动作队列空了没」：空了就按顺序播下一个待机动作（顺序轮播，不是库那种随机）。
  // 另用各文件自己的 `Meta.Duration` 做**兜底超时** —— 某个文件若因缺 `Meta.Duration` 等原因
  // 永远不结束（队列一直非空），超过预期时长就**强制**切下一个，不会卡在那一个动作上。

  // **每个注入动作的时长**（`Meta.Duration` 秒 → 毫秒；读不到给 3s 保守值）。两处都用：
  // ① 循环播放的兜底超时 ② 设置窗「进度条」（用户 2026-10-10：每个动画都有持续时间）。
  const durMs: number[] = await Promise.all(
    extraMotions.map(async m => {
      try {
        const r = await fetch(convertFileSrc(m.file));
        const j = (await r.json()) as { Meta?: { Duration?: unknown } };
        const d = Number(j?.Meta?.Duration);
        return Number.isFinite(d) && d > 0 ? d * 1000 : 3000;
      } catch {
        return 3000;
      }
    }),
  );

  // **进度上报**（2026-10-10）：引擎知道「我们刚播了哪个注入动作、从何时开始」，按它的
  // `Meta.Duration` 折算进度，定期回调给桌宠窗 → 设置窗画进度条。**只在真的播着时上报**。
  let curIndex = -1; // 当前由**我们**播的注入动作下标（-1 = 没在播 / 不是我们播的）
  let curStart = 0;
  let chainStepIdx = -1; // 由动作链驱动时是第几步；单动作播放时 -1
  let chainStepTotal = 0;
  const markStart = (index: number) => {
    curIndex = index;
    curStart = performance.now();
  };
  const reportProgress = (now: number) => {
    const cb = opts.onMotionProgress;
    if (!cb) return;
    if (curIndex < 0) {
      cb(null);
      return;
    }
    const total = durMs[curIndex] ?? 0;
    if (!(total > 0)) {
      cb(null);
      return;
    }
    // 循环播放时进度条每轮从头走：取模。
    const elapsed = Math.max(0, (now - curStart) % total);
    cb({
      index: curIndex,
      elapsedMs: elapsed,
      totalMs: total,
      chainStep: chainStepIdx,
      stepTotalMs: chainStepTotal,
    });
  };

  const mm = model.internalModel.motionManager as unknown as {
    isFinished(): boolean;
    state?: { currentGroup?: string; currentIndex?: number };
  };

  if (idleList.length) {
    let cursor = 0;
    let lastIndex = -1; // 上一次由**我们**播的循环动作（`Lunac` 组里的下标）
    let startedAt = 0;
    let expectMs = 0;
    let lastTry = 0;
    let lastIssue = 0;
    let driveErrs = 0;
    app.ticker.add(() => {
      // ⚠️ **本回调绝不允许抛**（2026-10-10 用户复现「一开循环就卡死」）：`@pixi/ticker` 的 `_tick`
      // 在 `update()` 抛异常后**就不再申请下一帧**（见 ㉕ 的根因分析）⇒ 整条渲染链永久停住。
      // 我们只给 `internalModel.update` 包了 try/catch，**够不着这里** —— 而库的 `model.motion()`
      // 在「组 / 序号不存在」「优先级与预约状态互相顶掉」等情况下是**同步抛**（不是 reject），
      // 那一下正好把 rAF 链掐断。整段兜住 + 记日志（最多 5 条）。
      try {
        const now = performance.now();
        if (!chainActive && now - lastTry >= 100) {
          lastTry = now;
          // 刚发过一条就歇 400ms：否则 `isFinished()` 若因「其实没开播」一直为 true，
          // 会退化成每 100ms 抢一次状态机（那正是把预约状态顶乱的来源）。
          if (now - lastIssue >= 400) {
            let idle = false;
            try {
              idle = mm.isFinished();
            } catch {
              idle = false;
            }
            // 「卡住」的判据必须**只在播的是我们那条循环动作**时成立 —— 否则用户自己播一条长动作
            // 会被我们误判成卡住、强行抢掉。`state.currentGroup/currentIndex` 是库的公开字段。
            const st = mm.state;
            const ours = !!st && st.currentGroup === MOTION_EXTRA_GROUP && st.currentIndex === lastIndex;
            const stuck = ours && startedAt > 0 && now - startedAt > expectMs + 1500;
            if (idle || stuck) {
              const k = cursor++ % idleList.length;
              lastIndex = idleList[k].index;
              startedAt = now;
              lastIssue = now;
              expectMs = durMs[lastIndex] ?? 3000;
              markStart(lastIndex);
              // 队列空 ⇒ NORMAL(2)，让用户自己播的动作优先；卡住 ⇒ FORCE(3) 抢过来切下一个。
              void model
                .motion(MOTION_EXTRA_GROUP, lastIndex, idle ? 2 : 3)
                .then(ok => {
                  if (!ok) {
                    startedAt = 0;
                    lastIndex = -1;
                  }
                })
                .catch(() => {
                  startedAt = 0;
                  lastIndex = -1;
                });
            }
          }
        }
      } catch (e) {
        if (driveErrs < 5) {
          driveErrs++;
          void invoke("log_frontend", {
            level: "error",
            message: `[pet engine] motion-cycle driver threw: ${e instanceof Error ? e.message : String(e)}`,
          }).catch(() => {});
        }
        // 出错就把「卡住」判据复位，下一拍重新试 —— 绝不在同一个判断上原地打死。
        startedAt = 0;
        lastIndex = -1;
      }
    });
  }

  // ── 动作链播放（2026-10-10，backlog ㉝②）──────────────────────────
  // 把 `steps`（已折算成「注入组下标 + 本步时长 + 硬停上限」）排在一条时间线上：
  // 每一步到点切下一步；本步动作若提前结束且还没到点，就**重播**它（凑满「循环播放时长」）。
  const chainState = { timer: 0 as unknown as number, active: false, step: 0, startedAt: 0, loop: false, steps: [] as { index: number; ms: number; hardMs: number }[] };
  // 供上面 idle 驱动器读的「链在跑」标志：链跑的时候 idle 驱动器让位（别抢状态机）。
  let chainActive = false;
  const stopChain = () => {
    if (chainState.timer) {
      window.clearInterval(chainState.timer);
      chainState.timer = 0;
    }
    chainState.active = false;
    chainActive = false;
    chainStepIdx = -1;
    chainStepTotal = 0;
  };
  const playChainStep = (i: number) => {
    const s = chainState.steps[i];
    if (!s) {
      // 整链循环（2026-10-11）：末步播完 ⇒ 从头再来；否则停（保持旧行为）。
      if (chainState.loop && chainState.active) {
        playChainStep(0);
        return;
      }
      stopChain();
      return;
    }
    chainState.step = i;
    chainState.startedAt = performance.now();
    chainStepIdx = i;
    chainStepTotal = s.ms;
    markStart(s.index);
    void model.motion(MOTION_EXTRA_GROUP, s.index, 3).catch(() => {});
  };
  const startChain = (steps: { index: number; ms: number; hardMs: number }[], loop = false) => {
    stopChain();
    if (!steps.length) return;
    chainState.steps = steps;
    chainState.loop = loop;
    chainState.active = true;
    chainActive = true;
    playChainStep(0);
    chainState.timer = window.setInterval(() => {
      try {
        if (!chainState.active) return;
        const now = performance.now();
        const s = chainState.steps[chainState.step];
        if (!s) {
          stopChain();
          return;
        }
        const advAt = Math.min(s.ms, s.hardMs > 0 ? s.hardMs : s.ms);
        const elapsed = now - chainState.startedAt;
        // 本步还没到点、但动作已经播完 ⇒ 重播它（「循环播放」这一半）。
        let finished = false;
        try {
          finished = mm.isFinished();
        } catch {
          finished = false;
        }
        if (elapsed < advAt && finished) {
          markStart(s.index);
          void model.motion(MOTION_EXTRA_GROUP, s.index, 3).catch(() => {});
          return;
        }
        if (elapsed >= advAt) playChainStep(chainState.step + 1);
      } catch (e) {
        void invoke("log_frontend", {
          level: "error",
          message: `[pet engine] chain player threw: ${e instanceof Error ? e.message : String(e)}`,
        }).catch(() => {});
        stopChain();
      }
    }, 80) as unknown as number;
  };

  // 进度上报：独立于 idle 驱动器（没有循环播放时也要上报）—— 节流到 ~12Hz。
  // `curIndex` 在动作播完、且没有动作链在跑时清掉（并**补发一次 `null`**），否则设置窗的
  // 进度条会停在最后一格 / 一直循环。`wasActive` 保证「由忙变闲」只补发一条 null。
  if (opts.onMotionProgress) {
    let lastRep = 0;
    let wasActive = false;
    app.ticker.add(() => {
      try {
        const now = performance.now();
        if (now - lastRep < 80) return;
        lastRep = now;
        if (!chainState.active && curIndex >= 0) {
          let done = false;
          try {
            done = mm.isFinished();
          } catch {
            done = false;
          }
          if (done) curIndex = -1;
        }
        const active = chainState.active || curIndex >= 0;
        if (active || wasActive) reportProgress(now);
        wasActive = active;
      } catch {
        /* 上报失败绝不能掐断 rAF 链 */
      }
    });
  }


  // ── 参数「保持」+ 表情（模型调试区，2026-10-09）────────────────────
  // 难点：模型的 Idle / 动作**每帧都会改写参数**，所以「拉条设一次」会被覆盖。
  // **做法**：内部维护 `heldParams`（Map<id, value>），在每帧 `beforeModelUpdate`
  // 回调里把这份值**全量覆盖**（weight=1）写回 coreModel。
  // **依据**（读 `pixi-live2d-display-lipsyncpatch/dist/cubism4.es.js` 的
  // `Cubism4InternalModel.update`，约 10867-10901 行）—— 一帧的既有次序是：
  //   ① emit("beforeMotionUpdate")
  //   ② motionManager.update（动作改写参数）
  //   ③ emit("afterMotionUpdate")
  //   ④ model.saveParameters()（把当前值存为「基准」）
  //   ⑤ expressionManager / eyeBlink / focus / breath / physics / pose 继续改写参数
  //   ⑥ emit("beforeModelUpdate")   ← 我们在这一步写回 heldParams
  //   ⑦ model.update()（**此刻的参数值被用于本帧渲染**）
  //   ⑧ model.loadParameters()（把 ④ 的基准还原，作为下一帧起点）
  // 只有夹在 ⑥⑦ 之间写回，我们的值才**真正决定本帧输出**。放在 ③ 不行：⑤ 的
  // expression/physics 仍会改写它，渲染用的是 ⑤ 之后的值；放在 ⑧ 之后则会被下一帧动作再覆盖。
  // 每帧重写 + ⑧ 的还原，使这份覆盖**不污染**模型内部状态 —— 这正是「保持」所需，
  // 松手（不再 setParameter）后动作会从自己的基准继续，不会留下残留。
  const heldParams = new Map<string, number>();
  /** 被「保持」的 ArtMesh / Part 不透明度（配件显隐）。与 `heldParams` 同理：每帧写回。 */
  const heldDrawables = new Map<string, number>();
  const heldParts = new Map<string, number>();
  /** 被「保持」的 **ArtMesh 乘法色 tint**（`id → {r,g,b}`，各 0..1）。与 `heldDrawables` 同理：
   *  每帧写回 Cubism Core 的 `drawables.multiplyColors`（框架每帧渲染时读它，见文件里
   *  `getDrawableMultiplyColor` 的调用链）。`null`（不在表里）= 原色 (1,1,1)。 */
  const heldDrawableColors = new Map<string, { r: number; g: number; b: number }>();
  /** **叠加的表情层**（key = 热键 id → 该 `.exp3.json` 的 `Parameters`）。多条可同时生效、
   *  互不冲掉，各自可单独取消 —— 对齐 VTS 的「Expression on/off」（2026-10-10）。 */
  const exprLayers = new Map<string, { id: string; value: number; blend: string }[]>();
  // coreModel 在类型里是宽泛的 `object`（Cubism4 运行时才有 setParameterValueById），
  // 这里局部收窄到我们真正用到的那一个方法；缺该参数的模型库自身会兜住（不抛错）。
  const internal = model.internalModel as unknown as {
    on(event: string, fn: () => void): void;
    off(event: string, fn: () => void): void;
    coreModel: { setParameterValueById(id: string, value: number, weight?: number): void };
    physics?: { evaluate?: (...a: unknown[]) => void };
  };

  // ── 运行时 id 表（热键 / 调试区的「可选目标」，2026-10-10）──────────
  // Cubism Core 的模型对象把 id 与对应数组分开放：`core._model.{parameters,drawables,parts}`
  // 各有 `ids`（字符串数组）与 `values/opacities`（Float32Array，按下标一一对应）。
  // **只读一次**（运行时不变），之后按 id→下标 查表即可。取不到时全部退化成空表（不抛错）。
  interface CoreArrays {
    parameters?: {
      ids?: unknown;
      values?: unknown;
      minimumValues?: unknown;
      maximumValues?: unknown;
      defaultValues?: unknown;
    };
    drawables?: { ids?: unknown; opacities?: unknown; multiplyColors?: unknown };
    parts?: { ids?: unknown; opacities?: unknown };
  }
  const coreArrays = (): CoreArrays | undefined =>
    (internal.coreModel as unknown as { _model?: CoreArrays })._model;
  const strArr = (a: unknown): string[] =>
    Array.isArray(a) ? (a as unknown[]).filter((x): x is string => typeof x === "string") : [];
  const f32 = (a: unknown): Float32Array | undefined =>
    a instanceof Float32Array ? a : undefined;

  const idIndex = (ids: string[]): Map<string, number> => {
    const m = new Map<string, number>();
    ids.forEach((s, i) => m.set(s, i));
    return m;
  };
  const paramIds = strArr(coreArrays()?.parameters?.ids);
  const drawableIds = strArr(coreArrays()?.drawables?.ids);
  const partIds = strArr(coreArrays()?.parts?.ids);
  const drawableIdx = idIndex(drawableIds);
  const partIdx = idIndex(partIds);
  const paramIdx = idIndex(paramIds);
  // 参数取值范围（调试区拉条用）：Cubism Core 的 `parameters` 里 `minimumValues` /
  // `maximumValues` / `defaultValues` 与 `ids` 同序。取不到就留空（调用方回退默认区间）。
  const paramRanges: Record<string, { min: number; max: number; def: number }> = {};
  {
    const p = coreArrays()?.parameters;
    const mn = f32(p?.minimumValues);
    const mx = f32(p?.maximumValues);
    const dv = f32(p?.defaultValues);
    if (mn && mx) {
      paramIds.forEach((id, i) => {
        if (i < mn.length && i < mx.length) {
          paramRanges[id] = { min: mn[i], max: mx[i], def: dv && i < dv.length ? dv[i] : 0 };
        }
      });
    }
  }

  /** **口型覆盖**（0..1；`null` = 放手让动作自己演嘴）。朗读期间由前端按宿主下发的
   *  包络写（见 `tts.rs` 的 `MOUTH_EVENT` 与 pet.ts 的 `driveMouth`）。 */
  let mouthOpen: number | null = null;
  /** 用哪条参数当「张嘴」：优先模型自己声明的 **LipSync** 参数（库的 `lipSyncIds`，
   *  与它内部做口型同步用的是同一份），取不到就退回 Cubism 约定名 `ParamMouthOpenY`。 */
  const mouthParamId =
    (model.internalModel.motionManager as unknown as { lipSyncIds?: string[] }).lipSyncIds?.[0] ||
    "ParamMouthOpenY";
  /** 该参数的满量程（包络是 0..1 的归一化幅度，写回时要乘回模型的量程）。 */
  const mouthMax = paramRanges[mouthParamId]?.max ?? 1;

  /** 每帧写回：① 叠加表情层（读当前值 → 按 Blend → 写回）② 用户「保持」的参数（weight=1 覆盖）
   *  ③ ArtMesh / Part 不透明度（配件显隐）。**必须在 `beforeModelUpdate` 打**：那是物理之后、
   *  渲染之前（见下面的次序注释）。顺序 = 表情层在**下**、用户保持在上（手动调的参数压过表情）。 */
  const readParamValue = (id: string): number | undefined => {
    const i = paramIdx.get(id);
    if (i === undefined) return undefined;
    const v = f32(coreArrays()?.parameters?.values);
    return v && i < v.length ? v[i] : undefined;
  };
  const applyHeld = () => {
    if (exprLayers.size) {
      for (const params of exprLayers.values()) {
        for (const p of params) {
          const cur = readParamValue(p.id);
          if (cur === undefined) continue;
          const nv =
            p.blend === "Add" ? cur + p.value : p.blend === "Multiply" ? cur * p.value : p.value;
          internal.coreModel.setParameterValueById(p.id, nv);
        }
      }
    }
    // **口型**（2026-10-10）：朗读期间由前端按宿主下发的包络写；`null` = 放手（动作自己的
    // 嘴部曲线继续）。位置与表情层同级（在用户「保持」之下）—— 用户手动按住嘴时以他为准。
    if (mouthOpen !== null && mouthParamId) {
      internal.coreModel.setParameterValueById(mouthParamId, mouthOpen * mouthMax, 1);
    }
    for (const [id, value] of heldParams) internal.coreModel.setParameterValueById(id, value);
    const ca = coreArrays();
    if (heldDrawables.size) {
      const op = f32(ca?.drawables?.opacities);
      if (op) for (const [id, v] of heldDrawables) {
        const i = drawableIdx.get(id);
        if (i !== undefined && i < op.length) op[i] = v;
      }
    }
    if (heldDrawableColors.size) {
      // **ArtMesh 乘法色（tint）**：`multiplyColors` 是**每 drawable 4 个 float（RGBA）**，
      // 与 `drawables.ids` 按下标对应（框架的 `getDrawableMultiplyColor(index)` 按 `index*4` 读，
      // 每帧渲染时调用 ⇒ 写这里就能决定本帧颜色）。A 恒写 1：tint 不改透明度（那归 `heldDrawables`）。
      const mc = f32(ca?.drawables?.multiplyColors);
      if (mc) for (const [id, c] of heldDrawableColors) {
        const i = drawableIdx.get(id);
        const o = i === undefined ? -1 : i * 4;
        if (o >= 0 && o + 3 < mc.length) {
          mc[o] = c.r;
          mc[o + 1] = c.g;
          mc[o + 2] = c.b;
          mc[o + 3] = 1;
        }
      }
    }
    if (heldParts.size) {
      const op = f32(ca?.parts?.opacities);
      if (op) for (const [id, v] of heldParts) {
        const i = partIdx.get(id);
        if (i !== undefined && i < op.length) op[i] = v;
      }
    }
  };
  internal.on("beforeModelUpdate", applyHeld);

  // ── 物理开关（2026-10-10）──────────────────────────────────────
  // 库在 `update()` 里调 `this.physics?.evaluate(coreModel, dt)`。**不改库内部字段**，
  // 只在实例上把 `evaluate` 包一层 —— 关掉时直接跳过，于是被物理驱动的输出参数不再被覆盖
  //（配合 `heldParams` 就能「热键/调参压过物理」）。包不住（没有物理）时整段静默跳过。
  let physicsOn = true;
  {
    const ph = internal.physics;
    if (ph && typeof ph.evaluate === "function") {
      const orig = ph.evaluate.bind(ph);
      ph.evaluate = (...args: unknown[]) => {
        if (physicsOn) orig(...args);
      };
    }
  }


  // 记录**未缩放**的设计尺寸：`model.width` 会随 scale 变，fit() 里不能反着读它
  const srcW = model.width || 1;
  const srcH = model.height || 1;
  // 「用户视图」= 整体缩放系数 + 相对**基准位置**的像素偏移。`fit()` 必须一并应用 ——
  // 否则 ResizeObserver 每次重算都会把用户调好的缩放/位置冲掉。滚轮锚点缩放把为锚住
  // 指针而产生的位移**并进偏移**（偏移本就只是「相对基准的位移」，合并后语义仍自洽）。
  let scale = opts.scale;
  let offX = opts.offsetX ?? 0;
  let offY = opts.offsetY ?? 0;
  // **整体旋转**（角度制）：绕 `model.anchor`（底边中心）转。与 scale / offset 各改各的。
  let rotDeg = opts.rotation ?? 0;

  const fit = () => {
    const w = Math.max(1, host.clientWidth);
    const h = Math.max(1, host.clientHeight);
    app.renderer.resize(w, h);
    const base = Math.min(w / srcW, h / srcH);
    model.scale.set(base * scale);
    model.anchor.set(0.5, 1); // 底边中心对齐 —— 桌宠站在窗口下沿
    model.position.set(w / 2 + offX, h + offY);
    model.rotation = (rotDeg * Math.PI) / 180;
  };
  fit();

  const ro = new ResizeObserver(() => fit());
  ro.observe(host);

  /** `destroy()` 必须**幂等**（2026-10-10 用户报障后加）：重画竞态下同一份实例可能被
   *  销毁两次（`paint()` 的「这次作废」分支 + `dropModel()`），而 pixi 的 `Application.destroy()`
   *  第二次会把内部已经置 null 的 `ticker._head` / `cancelResize` 再摸一遍 ⇒ 抛
   *  `Cannot read properties of null (reading 'next')` / `this.cancelResize is not a function`
   *  （桌宠窗日志里那两条 unhandledrejection 就是它）。 */
  let destroyed = false;

  return {
    setScale(next: number) {
      scale = next;
      fit();
    },
    setOffset(dx: number, dy: number) {
      offX = dx;
      offY = dy;
      fit();
    },
    setRotation(deg: number) {
      rotDeg = Number.isFinite(deg) ? deg : 0;
      fit();
    },
    zoomAtPointer(factor: number, px: number, py: number) {
      const w = Math.max(1, host.clientWidth);
      const h = Math.max(1, host.clientHeight);
      const base = Math.min(w / srcW, h / srcH);
      // 夹在配置认可的区间：越界的值下次 loadConfig 会被判非法、静默跳回 1。
      const next = Math.min(PET_SCALE_MAX, Math.max(PET_SCALE_MIN, scale * factor));
      const s0 = base * scale;
      const s1 = base * next;
      // 锚点下的像素不动 ⇒ 位置增量 = (锚点屏幕坐标 - 当前位置)·(1 - s1/s0)（推导：
      // 同一 local 点两次映射到同一屏幕点，两式相减）。位置 = 基准 + 偏移，故直接并进偏移。
      const r = s1 / s0;
      offX += (px - (w / 2 + offX)) * (1 - r);
      offY += (py - (h + offY)) * (1 - r);
      scale = next;
      fit();
      return { scale, offsetX: offX, offsetY: offY };
    },
    setRunning(on: boolean) {
      if (on) app.start();
      else app.stop();
    },
    setParameter(id: string, value: number) {
      if (!id) return;
      heldParams.set(id, value);
      // 立即写一次：窗口隐藏（ticker 停）时也立刻落到模型上，不必干等下一帧
      internal.coreModel.setParameterValueById(id, value);
    },
    getParameter(id: string): number {
      if (!id) return 0;
      // 优先读运行时 Core 那份（动作 / 物理刚写过的值）；拿不到就退回「保持」的那份。
      const i = paramIdx.get(id);
      const v = f32(coreArrays()?.parameters?.values);
      if (i !== undefined && v && i < v.length) return v[i];
      return heldParams.get(id) ?? 0;
    },
    setExpression(id: string) {
      // 库的 `Live2DModel.expression()` 按 `definitions[].Name` 查表（源码
      // `Cubism4ExpressionManager.getExpressionIndex`），并做平滑过渡；
      // 约定空串 = 还原由调用方（桌宠窗）分流，不会走到这里。
      void model.expression(id).catch(() => {});
    },
    resetExpressions() {
      model.internalModel.motionManager.expressionManager?.resetExpression();
    },
    setExpressionLayer(key: string, params: { id: string; value: number; blend: string }[] | null) {
      if (!key) return;
      if (params && params.length) exprLayers.set(key, params);
      else exprLayers.delete(key);
    },
    setMouthOpen(v: number | null) {
      // `null` = 放手（动作 / 物理自己演嘴）；数值夹在 0..1（0 = 闭嘴、1 = 满量程）。
      mouthOpen = v === null || !Number.isFinite(v) ? null : Math.max(0, Math.min(1, v));
    },
    // ── 热键能力（2026-10-10）────────────────────────────────────
    playMotion(group: string, index?: number) {
      if (!group) return;
      // 库的 `Live2DModel.motion()` 按 `Motions[组名][序号]` 取；组不存在 / 越界由它自己兜住。
      void model.motion(group, index ?? 0).catch(() => {});
      // 是我们注入的组 ⇒ 记下起点，进度条才画得出来（用户 2026-10-10）。
      if (group === MOTION_EXTRA_GROUP && typeof index === "number") markStart(index);
    },
    /** **播一条动作链**：`steps` 已折算成「注入组下标 + 本步时长（ms）+ 硬停上限（ms）」。
     *  `loop`（2026-10-11）= 整链循环：末步播完从头再来（缺省 false = 播完即停）。 */
    playChain(steps: { index: number; ms: number; hardMs: number }[], loop = false) {
      startChain(steps, loop);
    },
    /** 停掉正在播的动作链（不碰其它动作）。 */
    stopChain() {
      stopChain();
    },
    setDrawableOpacity(id: string, value: number) {
      if (!id) return;
      const i = drawableIdx.get(id);
      if (i === undefined) return; // 模型里没有这个 ArtMesh：静默忽略
      heldDrawables.set(id, value);
      const op = f32(coreArrays()?.drawables?.opacities);
      if (op && i < op.length) op[i] = value; // 立即写一次（窗口隐藏时也生效）
    },
    setDrawableColor(id: string, rgb: { r: number; g: number; b: number } | null) {
      if (!id) return;
      const i = drawableIdx.get(id);
      if (i === undefined) return; // 模型里没有这个 ArtMesh：静默忽略
      if (rgb) heldDrawableColors.set(id, { r: rgb.r, g: rgb.g, b: rgb.b });
      else heldDrawableColors.delete(id); // 还原原色：不再每帧覆盖
      // 立即写一次（窗口隐藏时也生效）：`rgb === null` 写回原色 (1,1,1)。
      const mc = f32(coreArrays()?.drawables?.multiplyColors);
      const o = i * 4;
      if (mc && o + 3 < mc.length) {
        const c = rgb ?? { r: 1, g: 1, b: 1 };
        mc[o] = c.r;
        mc[o + 1] = c.g;
        mc[o + 2] = c.b;
        mc[o + 3] = 1;
      }
    },
    setPartOpacity(id: string, value: number) {
      if (!id) return;
      const i = partIdx.get(id);
      if (i === undefined) return;
      heldParts.set(id, value);
      const op = f32(coreArrays()?.parts?.opacities);
      if (op && i < op.length) op[i] = value;
    },
    setPhysicsEnabled(on: boolean) {
      physicsOn = on;
    },
    listCoreIds() {
      return { parameters: paramIds, drawables: drawableIds, parts: partIds, parameterRanges: paramRanges };
    },
    destroy() {
      if (destroyed) return; // 幂等：见上面 `destroyed` 的注释（pixi 二次 destroy 会抛）
      destroyed = true;
      stopChain(); // 别让动作链的 interval 在实例销毁后还去碰 model
      internal.off("beforeModelUpdate", applyHeld);
      exprLayers.clear();
      heldDrawableColors.clear();
      ro.disconnect();
      // ⚠️ **pixi 的 `Application.destroy()` 自己也可能抛**（2026-10-10 用户报「点开启依旧卡死」）：
      // 除了「同一句柄被销毁两次」之外，pixi 内部还会对**同一个 ticker** 再销毁一次
      //（`Ticker.destroy()` 读已置 null 的 `_head.next` ⇒ `Cannot read properties of null (reading 'next')`，
      //  宿主日志里每次都出现）。这个异常**必须吞掉**：调用方 `paint()` 在挂新模型**之前**先
      // `dropModel()`，一旦这里抛出去，`paint()` 就会在挂载前中断 ⇒ 新模型永远挂不上、
      // 用户看到的正是「一开循环就卡死」（其实是重挂被中断、旧模型已拆）。
      try {
        app.destroy(true, { children: true, texture: true, baseTexture: true });
      } catch (e) {
        void invoke("log_frontend", {
          level: "warn",
          message: `[pet engine] app.destroy threw (已吞掉，避免中断重挂): ${e instanceof Error ? e.message : String(e)}`,
        }).catch(() => {});
      }
      // ⚠️ **兜底移除画布**（2026-10-10 用户报「开循环播放后下方多出一个模型、原位置那个停住」）：
      // pixi 的 `Application.destroy(removeView=true)` 内部次序是「先 `ticker.destroy()`、**再** removeView」——
      // 而 `Ticker.destroy()` 正是会抛的那一步（见上），异常一抛，removeView 就**永远执行不到**
      // ⇒ 旧画布留在 DOM 里、新画布 append 到它下面 ⇒ 两个模型上下排列（旧的那个因 ticker 已停而「暂停」）。
      // 这里无条件把画布从父节点摘掉，与 destroy 是否抛错无关。
      canvas.remove();
    },
  };
}
