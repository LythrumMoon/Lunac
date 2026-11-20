export * from "./contract.js";
export { validateSourceManifest } from "./manifest.js";
export {
  HOST_API_VERSION,
  findHost,
  hasHostV3,
  requireHostV3,
  type EngineApi,
  type HostCapability,
  type HostRequestInit,
  type HostResponse,
  type LunacHostV3,
  type MediaApi,
  type NetApi,
  type PinyinApi,
  type PinyinEntry,
  type SniffApi,
  type StorageApi,
} from "./host.js";
export { matchPinyin, normalizeLatin, type PinyinMatch } from "./pinyin.js";
export {
  createMockEngine,
  createMockHost,
  createMockNet,
  createMockSniff,
  createMockStorage,
  type MockHostOptions,
} from "./mock-host.js";
export {
  createNetClient,
  type NetClient,
  type NetClientOptions,
} from "./net-client.js";
export {
  applyRegex,
  buildRequestUrl,
  extractHtmlRows,
  extractJsonRows,
  interpolate,
  jsonPathFirst,
  jsonPathValues,
  selectElements,
  type RuleVars,
} from "./evaluate.js";
export {
  createDeclarativeSource,
  type DeclarativeSourceDeps,
  type DeclarativeSourceInput,
} from "./executor.js";
export {
  createJsSource,
  toRspError,
  type JsSourceInput,
} from "./js-source.js";
export {
  createComicSource,
  looksLikeComic,
  type ComicSourceInput,
} from "./comic-source.js";
export {
  createVideoSource,
  looksLikeVideo,
  videoHasExplore,
  videoUnsupportedReason,
  type VideoSourceInput,
  type VideoUnsupportedReason,
} from "./video-source.js";
export {
  convertMangaRule,
  type MangaApiConfig,
  type MangaConversion,
  type MangaRule,
} from "./manga.js";
export {
  compareVersion,
  fromMangaIndex,
  fromVideoIndex,
  fromComicIndex,
  fromCmsIndex,
  mergeRegistry,
  normalizeLegacyId,
  normalizeLegacyUpstream,
  parseRegistry,
  type RegistryBundle,
  type RegistryEntry,
  type Upstream,
} from "./registry.js";
export {
  htmlToText,
  parseEpub,
  type EpubBook,
  type EpubChapter,
} from "./epub.js";
