// 漫画源兼容层（2026-10-18）。
//
// 目标：让 `class X extends ComicSource` 形态的上游 JS 源能在宿主 QuickJS 沙箱里跑
// **搜索 / 章节 / 图片**三条主链路。这里是**接口翻译**，不是复刻 漫画源：
//   · Network.*   → 宿主 host.request（同步网络 + Promise 外壳）
//   · HtmlDocument → 本文件内的轻量 DOM + CSS 选择器（无宿主依赖，移动端可复用）
//   · Comic / ComicDetails / ComicSource → 数据与设置的最小实现
// 未实现：账号、收藏、分类、评论、图片后处理、GBK 编码（调用时给出可读错误）。
//
// 注意：下面是**两段**注入沙箱的 JS 源码（String.raw），不要写 `${}`。
// 组合顺序：PRELUDE → 上游脚本 → RUNNER（上游脚本定义 class 时全局已就绪）。

import { GB2312_CHARS } from "./gbk-table.js";

export const COMIC_PRELUDE = String.raw`
// ── 宿主能力包装 ────────────────────────────────────────────────
// 清单 headers（UA / Referer / Cookie 等）作为默认值，单次请求里同名键优先。
var __lunac_default_headers =
  typeof args !== "undefined" && args && args.defaultHeaders ? args.defaultHeaders : {};
// 来源自己的持久化数据（登录令牌 / Cookie 等）：宿主在每次 eval 前读入、结束后写回。
var __lunac_stored_data =
  typeof args !== "undefined" && args && args.storedData !== undefined ? args.storedData : undefined;
var __lunac_data_dirty = false;
var __lunac_request = function (method, url, headers, data) {
  // 上游两种写法都存在：
  //   Network.get(url, { "User-Agent": "…" })          → 第二参就是头对象
  //   Network.get(url, { headers: { "User-Agent": … } }) → 第二参是 options
  // 宿主只接受字符串头；把 options 原样透传会变成「header 值是个对象」，
  // Rust 侧 serde 直接拒掉整次请求（invalid type: map, expected a string）。
  if (
    headers &&
    typeof headers === "object" &&
    headers.headers &&
    typeof headers.headers === "object"
  ) {
    if ((data === undefined || data === null) && headers.body !== undefined) {
      data = headers.body;
    }
    headers = headers.headers;
  }
  var merged = {};
  for (var dh in __lunac_default_headers) {
    if (Object.prototype.hasOwnProperty.call(__lunac_default_headers, dh)) {
      merged[dh] = __lunac_default_headers[dh];
    }
  }
  if (headers) {
    for (var key in headers) {
      if (Object.prototype.hasOwnProperty.call(headers, key)) {
        var value = headers[key];
        if (value === undefined || value === null) continue;
        merged[key] = typeof value === "object" ? JSON.stringify(value) : String(value);
      }
    }
  }
  var hasUA = false;
  for (var hk in merged) {
    if (String(hk).toLowerCase() === "user-agent") hasUA = true;
  }
  if (!hasUA) {
    merged["User-Agent"] =
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";
  }
  var options = { url: String(url), method: method, headers: merged };
  if (data !== undefined && data !== null) {
    if (typeof data === "string") {
      options.body = data;
    } else if (
      data instanceof ArrayBuffer ||
      (typeof ArrayBuffer !== "undefined" && typeof ArrayBuffer.isView === "function" && ArrayBuffer.isView(data))
    ) {
      // 上游常用 Convert.encodeUtf8 / encodeGbk 传字节：body 给可读兜底，
      // bodyBase64 让宿主按原始字节发送（GBK 表单不会变成乱码）。
      if (data instanceof ArrayBuffer) {
        options.body = __lunac_utf8_decode(data);
        options.bodyBase64 = __lunac_b64encode(data);
      } else {
        var byteBuffer = data.buffer.slice(
          data.byteOffset || 0,
          (data.byteOffset || 0) + data.byteLength,
        );
        options.body = __lunac_utf8_decode(byteBuffer);
        options.bodyBase64 = __lunac_b64encode(byteBuffer);
      }
    } else if (typeof data === "object") {
      var contentType = "";
      for (var ck in merged) {
        if (String(ck).toLowerCase() === "content-type") contentType = String(merged[ck]).toLowerCase();
      }
      if (contentType.indexOf("json") >= 0) {
        options.body = JSON.stringify(data);
      } else {
        var pairs = [];
        for (var dk in data) {
          if (!Object.prototype.hasOwnProperty.call(data, dk)) continue;
          var value = data[dk];
          if (Array.isArray(value)) {
            for (var vi = 0; vi < value.length; vi++) {
              pairs.push(encodeURIComponent(dk) + "=" + encodeURIComponent(value[vi] == null ? "" : value[vi]));
            }
          } else {
            pairs.push(encodeURIComponent(dk) + "=" + encodeURIComponent(value == null ? "" : value));
          }
        }
        options.body = pairs.join("&");
        if (!contentType) merged["Content-Type"] = "application/x-www-form-urlencoded";
      }
    } else {
      options.body = String(data);
    }
  }
  var raw = host.request(JSON.stringify(options));
  var envelope;
  try {
    envelope = JSON.parse(raw);
  } catch (error) {
    throw new Error("host.request 返回了非 JSON");
  }
  if (!envelope || envelope.ok !== true) throw new Error((envelope && envelope.error) || "请求失败");
  return envelope.response;
};

var Network = {
  get: function (url, headers) {
    return __lunac_request("GET", url, headers, null);
  },
  post: function (url, headers, data) {
    return __lunac_request("POST", url, headers, data);
  },
  put: function (url, headers, data) {
    return __lunac_request("PUT", url, headers, data);
  },
  patch: function (url, headers, data) {
    return __lunac_request("PATCH", url, headers, data);
  },
  delete: function (url, headers) {
    return __lunac_request("DELETE", url, headers, null);
  },
  sendRequest: function (method, url, headers, data) {
    return __lunac_request(method, url, headers, data);
  },
  fetchBytes: function (method, url, headers, data) {
    var res = __lunac_request(method, url, headers, data);
    // 宿主对非 UTF-8 响应回传原始字节（bodyBase64）：交给上游按 GB2312 解码。
    if (res && res.bodyBase64) {
      return { status: res.status, headers: res.headers, body: __lunac_b64decode(res.bodyBase64) };
    }
    return res;
  },
};

function fetch(url, options) {
  var opts = options || {};
  var res = __lunac_request(opts.method || "GET", url, opts.headers, opts.body);
  return Promise.resolve({
    ok: res.status >= 200 && res.status < 300,
    status: res.status,
    statusText: "",
    headers: res.headers || {},
    text: function () { return Promise.resolve(res.body); },
    json: function () { return Promise.resolve(JSON.parse(res.body)); },
    arrayBuffer: function () { return Promise.resolve(__lunac_utf8(res.body)); },
  });
}

function setTimeout(callback) {
  if (typeof callback === "function") {
    try { callback(); } catch (error) { /* 沙箱没有异步定时器，直接执行 */ }
  }
  return 0;
}
function clearTimeout() {}
function setInterval() { return { cancel: function () {} }; }
function clearInterval() {}
function randomInt(min, max) {
  var low = Number(min);
  var high = Number(max);
  if (!isFinite(low)) low = 0;
  if (!isFinite(high)) high = low;
  if (high < low) { var swap = low; low = high; high = swap; }
  return Math.floor(Math.random() * (Math.floor(high) - Math.ceil(low) + 1)) + Math.ceil(low);
}
function __lunac_utf8(value) {
  var text = String(value == null ? "" : value);
  var bytes = [];
  for (var i = 0; i < text.length; i++) {
    var code = text.charCodeAt(i);
    if (code < 0x80) bytes.push(code);
    else if (code < 0x800) bytes.push(0xc0 | (code >> 6), 0x80 | (code & 63));
    else if (code < 0xd800 || code >= 0xe000) bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 63), 0x80 | (code & 63));
    else {
      i++;
      var next = text.charCodeAt(i);
      var point = 0x10000 + (((code & 0x3ff) << 10) | (next & 0x3ff));
      bytes.push(0xf0 | (point >> 18), 0x80 | ((point >> 12) & 63), 0x80 | ((point >> 6) & 63), 0x80 | (point & 63));
    }
  }
  var buffer = new ArrayBuffer(bytes.length);
  var view = new Uint8Array(buffer);
  for (var j = 0; j < bytes.length; j++) view[j] = bytes[j];
  return buffer;
}

function __lunac_utf8_decode(buffer) {
  var view = new Uint8Array(buffer);
  var out = "";
  for (var i = 0; i < view.length;) {
    var b0 = view[i++];
    if (b0 < 0x80) {
      out += String.fromCharCode(b0);
    } else if (b0 < 0xe0) {
      var b1 = view[i++] & 63;
      out += String.fromCharCode(((b0 & 31) << 6) | b1);
    } else if (b0 < 0xf0) {
      var b2 = view[i++] & 63;
      var b3 = view[i++] & 63;
      out += String.fromCharCode(((b0 & 15) << 12) | (b2 << 6) | b3);
    } else {
      var b4 = view[i++] & 63;
      var b5 = view[i++] & 63;
      var b6 = view[i++] & 63;
      var point = (((b0 & 7) << 18) | (b4 << 12) | (b5 << 6) | b6) - 0x10000;
      out += String.fromCharCode(0xd800 + (point >> 10), 0xdc00 + (point & 1023));
    }
  }
  return out;
}

var __B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
function __lunac_b64encode(buffer) {
  var view = new Uint8Array(buffer);
  var out = "";
  for (var i = 0; i < view.length; i += 3) {
    var b0 = view[i];
    var b1 = i + 1 < view.length ? view[i + 1] : 0;
    var b2 = i + 2 < view.length ? view[i + 2] : 0;
    out += __B64.charAt(b0 >> 2);
    out += __B64.charAt(((b0 & 3) << 4) | (b1 >> 4));
    out += i + 1 < view.length ? __B64.charAt(((b1 & 15) << 2) | (b2 >> 6)) : "=";
    out += i + 2 < view.length ? __B64.charAt(b2 & 63) : "=";
  }
  return out;
}
function __lunac_b64decode(value) {
  var clean = String(value).replace(/[^A-Za-z0-9+/=]/g, "");
  var bytes = [];
  for (var i = 0; i < clean.length; i += 4) {
    var c0 = __B64.indexOf(clean.charAt(i));
    var c1 = __B64.indexOf(clean.charAt(i + 1));
    var c2 = __B64.indexOf(clean.charAt(i + 2));
    var c3 = __B64.indexOf(clean.charAt(i + 3));
    if (c0 < 0 || c1 < 0) continue;
    bytes.push((c0 << 2) | (c1 >> 4));
    if (c2 >= 0) bytes.push(((c1 & 15) << 4) | (c2 >> 2));
    if (c3 >= 0) bytes.push(((c2 & 3) << 6) | c3);
  }
  var buffer = new ArrayBuffer(bytes.length);
  var view = new Uint8Array(buffer);
  for (var j = 0; j < bytes.length; j++) view[j] = bytes[j];
  return buffer;
}

// ── HTML：容错解析 + CSS 选择器子集 ─────────────────────────────
var __VOID = { area: 1, base: 1, br: 1, col: 1, embed: 1, hr: 1, img: 1, input: 1, link: 1, meta: 1, param: 1, source: 1, track: 1, wbr: 1 };
var __RAW = { script: 1, style: 1 };
var __ENTITIES = {
  amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ",
  mdash: "\u2014", ndash: "\u2013", hellip: "\u2026",
  lsquo: "\u2018", rsquo: "\u2019", ldquo: "\u201c", rdquo: "\u201d",
};

function __decode(value) {
  return String(value == null ? "" : value).replace(/&(#x?[0-9a-f]+|[a-z]+);/gi, function (match, name) {
    if (name.charAt(0) === "#") {
      var code = name.charAt(1) === "x" || name.charAt(1) === "X"
        ? parseInt(name.slice(2), 16)
        : parseInt(name.slice(1), 10);
      return isFinite(code) ? String.fromCodePoint(code) : match;
    }
    var named = __ENTITIES[String(name).toLowerCase()];
    return named !== undefined ? named : match;
  });
}

function __parseAttrs(text) {
  var attrs = {};
  var re = /([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
  var match;
  while ((match = re.exec(text))) {
    var name = String(match[1]).toLowerCase();
    var value = match[2] !== undefined ? match[2] : match[3] !== undefined ? match[3] : match[4] !== undefined ? match[4] : "";
    attrs[name] = __decode(value);
  }
  return attrs;
}

function __parseHtml(source) {
  var text = String(source == null ? "" : source);
  var root = { type: "document", tag: "#document", attrs: {}, children: [], parent: null, text: "" };
  var stack = [root];

  function appendText(value) {
    if (!value) return;
    stack[stack.length - 1].children.push({ type: "text", tag: "#text", attrs: {}, children: [], parent: stack[stack.length - 1], text: value });
  }
  function closeTag(tag) {
    for (var i = stack.length - 1; i > 0; i--) {
      if (stack[i].tag === tag) {
        stack.length = i;
        return;
      }
    }
  }

  var i = 0;
  while (i < text.length) {
    var lt = text.indexOf("<", i);
    if (lt < 0) { appendText(text.slice(i)); break; }
    if (lt > i) appendText(text.slice(i, lt));
    if (text.slice(lt, lt + 4) === "<!--") {
      var commentEnd = text.indexOf("-->", lt + 4);
      i = commentEnd < 0 ? text.length : commentEnd + 3;
      continue;
    }
    if (text.charAt(lt + 1) === "!" || text.charAt(lt + 1) === "?") {
      var declEnd = text.indexOf(">", lt + 2);
      i = declEnd < 0 ? text.length : declEnd + 1;
      continue;
    }
    var gt = text.indexOf(">", lt + 1);
    if (gt < 0) { appendText(text.slice(lt)); break; }
    var raw = text.slice(lt + 1, gt).trim();
    if (raw.charAt(0) === "/") {
      closeTag(raw.slice(1).trim().toLowerCase());
      i = gt + 1;
      continue;
    }
    var selfClosing = raw.charAt(raw.length - 1) === "/";
    var body = selfClosing ? raw.slice(0, -1).trim() : raw;
    var space = body.search(/\s/);
    var tag = (space < 0 ? body : body.slice(0, space)).toLowerCase();
    if (!tag) { i = gt + 1; continue; }
    var node = {
      type: "element",
      tag: tag,
      attrs: __parseAttrs(space < 0 ? "" : body.slice(space)),
      children: [],
      parent: stack[stack.length - 1],
      text: "",
    };
    stack[stack.length - 1].children.push(node);
    i = gt + 1;
    if (selfClosing || __VOID[tag]) continue;
    if (__RAW[tag]) {
      var close = text.toLowerCase().indexOf("</" + tag, i);
      var content = close < 0 ? text.slice(i) : text.slice(i, close);
      if (content) {
        node.children.push({ type: "text", tag: "#text", attrs: {}, children: [], parent: node, text: content });
      }
      if (close < 0) { i = text.length; } else {
        var closeEnd = text.indexOf(">", close);
        i = closeEnd < 0 ? text.length : closeEnd + 1;
      }
      continue;
    }
    stack.push(node);
  }
  return root;
}

function __isElement(node) {
  return !!node && node.type === "element";
}
function __textOf(node) {
  if (!node) return "";
  if (node.type === "text") return node.text || "";
  var out = "";
  for (var i = 0; i < node.children.length; i++) out += __textOf(node.children[i]);
  return out;
}
function __trimmedText(node) {
  return __decode(__textOf(node)).replace(/\s+/g, " ").trim();
}
function __serialize(node) {
  if (node.type === "text") return node.text || "";
  var out = "<" + node.tag;
  for (var key in node.attrs) {
    if (Object.prototype.hasOwnProperty.call(node.attrs, key)) {
      out += " " + key + '="' + String(node.attrs[key]).replace(/"/g, "&quot;") + '"';
    }
  }
  out += ">";
  if (__VOID[node.tag]) return out;
  for (var i = 0; i < node.children.length; i++) out += __serialize(node.children[i]);
  return out + "</" + node.tag + ">";
}

function __parseCompound(token) {
  var compound = { tag: null, id: null, classes: [], attrs: [], pseudos: [] };
  var re = /([.#]?[\w-]+|\[[^\]]+\]|:[\w-]+(?:\([^)]*\))?|\*)/g;
  var match;
  while ((match = re.exec(token))) {
    var piece = String(match[1]);
    if (piece === "*") { compound.tag = "*"; continue; }
    var first = piece.charAt(0);
    if (first === ".") { compound.classes.push(piece.slice(1)); continue; }
    if (first === "#") { compound.id = piece.slice(1); continue; }
    if (first === "[") {
      var body = piece.slice(1, -1);
      var am = /^([\w:-]+)\s*(?:([~^$*]?=)\s*(?:"([^"]*)"|'([^']*)'|([^\s]+)))?$/.exec(body);
      if (am) {
        compound.attrs.push({
          name: String(am[1]).toLowerCase(),
          op: am[2] || "",
          value: am[3] !== undefined ? am[3] : am[4] !== undefined ? am[4] : am[5] !== undefined ? am[5] : "",
        });
      }
      continue;
    }
    if (first === ":") {
      var pm = /^:([\w-]+)(?:\(([^)]*)\))?$/.exec(piece);
      if (pm) compound.pseudos.push({ name: String(pm[1]).toLowerCase(), arg: pm[2] ? parseInt(pm[2], 10) : 0 });
      continue;
    }
    if (!compound.tag) compound.tag = piece.toLowerCase();
  }
  return compound;
}

function __parseSelector(selector) {
  var groups = String(selector).split(",");
  var parsed = [];
  for (var g = 0; g < groups.length; g++) {
    var raw = groups[g].trim().replace(/\s*>\s*/g, " > ");
    if (!raw) continue;
    var tokens = raw.split(/\s+/);
    var parts = [];
    var pending = "";
    for (var t = 0; t < tokens.length; t++) {
      var token = tokens[t];
      if (!token) continue;
      if (token === ">") { pending = ">"; continue; }
      parts.push({ combinator: parts.length === 0 ? "" : (pending || " "), compound: __parseCompound(token) });
      pending = "";
    }
    if (parts.length) parsed.push(parts);
  }
  return parsed;
}

function __matchesCompound(node, compound) {
  if (!__isElement(node)) return false;
  if (compound.tag && compound.tag !== "*" && node.tag !== compound.tag) return false;
  if (compound.id && node.attrs.id !== compound.id) return false;
  if (compound.classes.length) {
    var classes = String(node.attrs["class"] || "").split(/\s+/);
    for (var i = 0; i < compound.classes.length; i++) {
      if (classes.indexOf(compound.classes[i]) < 0) return false;
    }
  }
  for (var a = 0; a < compound.attrs.length; a++) {
    var attr = compound.attrs[a];
    var value = node.attrs[attr.name];
    if (value === undefined) return false;
    if (attr.op === "=" && value !== attr.value) return false;
    if (attr.op === "~=" && (" " + value + " ").indexOf(" " + attr.value + " ") < 0) return false;
    if (attr.op === "^=" && value.indexOf(attr.value) !== 0) return false;
    if (attr.op === "$=" && value.slice(-attr.value.length) !== attr.value) return false;
    if (attr.op === "*=" && value.indexOf(attr.value) < 0) return false;
  }
  for (var p = 0; p < compound.pseudos.length; p++) {
    var pseudo = compound.pseudos[p];
    var siblings = [];
    var parent = node.parent;
    if (parent) {
      for (var s = 0; s < parent.children.length; s++) {
        if (__isElement(parent.children[s])) siblings.push(parent.children[s]);
      }
    }
    var index = siblings.indexOf(node);
    if (pseudo.name === "first-child" && index !== 0) return false;
    if (pseudo.name === "last-child" && index !== siblings.length - 1) return false;
    if (pseudo.name === "nth-child" && index !== pseudo.arg - 1) return false;
  }
  return true;
}

function __matchesChain(node, parts, index) {
  if (index < 0) return true;
  if (!__matchesCompound(node, parts[index].compound)) return false;
  if (index === 0) return true;
  var combinator = parts[index].combinator;
  var parent = node.parent;
  if (combinator === ">") return __isElement(parent) && __matchesChain(parent, parts, index - 1);
  while (parent) {
    if (__isElement(parent) && __matchesChain(parent, parts, index - 1)) return true;
    parent = parent.parent;
  }
  return false;
}

function __collect(node, groups, out) {
  for (var i = 0; i < node.children.length; i++) {
    var child = node.children[i];
    if (!__isElement(child)) continue;
    for (var g = 0; g < groups.length; g++) {
      if (__matchesChain(child, groups[g], groups[g].length - 1)) { out.push(child); break; }
    }
    __collect(child, groups, out);
  }
  return out;
}

function __wrap(node) {
  if (node.__wrap) return node.__wrap;
  node.__wrap = node.type === "element" ? new HtmlElement(node) : new HtmlNode(node);
  return node.__wrap;
}

function HtmlNode(node) {
  this.node = node;
}
Object.defineProperty(HtmlNode.prototype, "text", {
  get: function () {
    if (this.node.type === "text") return __decode(this.node.text);
    return __trimmedText(this.node);
  },
});
Object.defineProperty(HtmlNode.prototype, "type", {
  get: function () { return this.node.type; },
});
HtmlNode.prototype.toElement = function () {
  return this.node.type === "element" ? __wrap(this.node) : null;
};

function HtmlElement(node) {
  HtmlNode.call(this, node);
}
HtmlElement.prototype = Object.create(HtmlNode.prototype);
HtmlElement.prototype.constructor = HtmlElement;
Object.defineProperty(HtmlElement.prototype, "attributes", {
  get: function () { return this.node.attrs; },
});
Object.defineProperty(HtmlElement.prototype, "children", {
  get: function () {
    var out = [];
    for (var i = 0; i < this.node.children.length; i++) {
      if (__isElement(this.node.children[i])) out.push(__wrap(this.node.children[i]));
    }
    return out;
  },
});
Object.defineProperty(HtmlElement.prototype, "nodes", {
  get: function () {
    var out = [];
    for (var i = 0; i < this.node.children.length; i++) out.push(__wrap(this.node.children[i]));
    return out;
  },
});
Object.defineProperty(HtmlElement.prototype, "classNames", {
  get: function () { return String(this.node.attrs["class"] || "").split(/\s+/).filter(Boolean); },
});
Object.defineProperty(HtmlElement.prototype, "id", {
  get: function () { return this.node.attrs.id !== undefined ? this.node.attrs.id : null; },
});
Object.defineProperty(HtmlElement.prototype, "localName", {
  get: function () { return this.node.tag; },
});
Object.defineProperty(HtmlElement.prototype, "innerHTML", {
  get: function () {
    var out = "";
    for (var i = 0; i < this.node.children.length; i++) out += __serialize(this.node.children[i]);
    return out;
  },
});
Object.defineProperty(HtmlElement.prototype, "parent", {
  get: function () {
    var parent = this.node.parent;
    return __isElement(parent) ? __wrap(parent) : null;
  },
});
Object.defineProperty(HtmlElement.prototype, "previousElementSibling", {
  get: function () {
    var parent = this.node.parent;
    if (!parent) return null;
    var list = parent.children;
    var index = list.indexOf(this.node);
    for (var i = index - 1; i >= 0; i--) if (__isElement(list[i])) return __wrap(list[i]);
    return null;
  },
});
Object.defineProperty(HtmlElement.prototype, "nextElementSibling", {
  get: function () {
    var parent = this.node.parent;
    if (!parent) return null;
    var list = parent.children;
    var index = list.indexOf(this.node);
    for (var i = index + 1; i < list.length; i++) if (__isElement(list[i])) return __wrap(list[i]);
    return null;
  },
});
HtmlElement.prototype.querySelector = function (selector) {
  var groups = __parseSelector(selector);
  if (!groups.length) return null;
  var found = __collect(this.node, groups, []);
  return found.length ? __wrap(found[0]) : null;
};
HtmlElement.prototype.querySelectorAll = function (selector) {
  var groups = __parseSelector(selector);
  if (!groups.length) return [];
  return __collect(this.node, groups, []).map(__wrap);
};

function HtmlDocument(html) {
  this.root = __parseHtml(html);
}
HtmlDocument.prototype.querySelector = function (selector) {
  var groups = __parseSelector(selector);
  if (!groups.length) return null;
  var found = __collect(this.root, groups, []);
  return found.length ? __wrap(found[0]) : null;
};
HtmlDocument.prototype.querySelectorAll = function (selector) {
  var groups = __parseSelector(selector);
  if (!groups.length) return [];
  return __collect(this.root, groups, []).map(__wrap);
};
HtmlDocument.prototype.getElementById = function (id) {
  return this.querySelector("#" + String(id));
};
HtmlDocument.prototype.dispose = function () {};

// ── 数据模型 / 基类 ─────────────────────────────────────────────
function __assign(target, source) {
  if (source) {
    for (var key in source) {
      if (Object.prototype.hasOwnProperty.call(source, key)) target[key] = source[key];
    }
  }
  return target;
}
function Comic(data) { __assign(this, data || {}); }
function ComicDetails(data) { __assign(this, data || {}); }
function Comment(data) { __assign(this, data || {}); }
function ImageLoadingConfig(data) { __assign(this, data || {}); }

var APP = { locale: "zh-CN", version: "1.6.2", isAndroid: false, isIOS: false, isWindows: true };

function ComicSource() {
  this.name = "";
  this.key = "";
  this.version = "0.0.0";
  this.url = "";
  this.settings = {};
  this.translation = {};
}
ComicSource.prototype.loadSetting = function (key) {
  if (
    this.__overrides &&
    Object.prototype.hasOwnProperty.call(this.__overrides, key) &&
    this.__overrides[key] !== undefined &&
    this.__overrides[key] !== ""
  ) {
    return this.__overrides[key];
  }
  if (this.__settings && Object.prototype.hasOwnProperty.call(this.__settings, key)) {
    return this.__settings[key];
  }
  var setting = this.settings && this.settings[key];
  return setting && setting.default !== undefined ? setting.default : undefined;
};
ComicSource.prototype.saveSetting = function (key, value) {
  this.__settings = this.__settings || {};
  this.__settings[key] = value;
};
ComicSource.prototype.loadData = function () { return __lunac_stored_data; };
ComicSource.prototype.saveData = function (value) {
  __lunac_stored_data = value;
  __lunac_data_dirty = true;
};
ComicSource.prototype.deleteData = function () {
  __lunac_stored_data = undefined;
  __lunac_data_dirty = true;
};
ComicSource.prototype.translate = function (key) {
  if (!this.translation) return key;
  var table = this.translation[APP.locale] || this.translation["zh-CN"] || this.translation["en"] || null;
  return table && table[key] !== undefined ? table[key] : key;
};
ComicSource.prototype.init = function () {};

// ── GBK 编码（GB2312 一级 + 二级汉字）────────────────────────────
// 表由 Node TextDecoder("gbk") 生成，按 GBK 码位顺序排列；QuickJS 没有 TextDecoder，
// 因此这里自带一张 6763 字的反查表。
var __GB2312_CHARS = "__GB2312_TABLE__";
var __GBK_MAP = null;
function __gbkMap() {
  if (__GBK_MAP) return __GBK_MAP;
  var map = {};
  var index = 0;
  function add(lead, trail) {
    if (index >= __GB2312_CHARS.length) return;
    var ch = __GB2312_CHARS.charAt(index++);
    if (map[ch] === undefined) map[ch] = [lead, trail];
  }
  for (var lead = 0xb0; lead <= 0xd7; lead++) {
    var max = lead === 0xd7 ? 0xf9 : 0xfe;
    for (var trail = 0xa1; trail <= max; trail++) add(lead, trail);
  }
  for (var lead = 0xd8; lead <= 0xf7; lead++) {
    for (var trail = 0xa1; trail <= 0xfe; trail++) add(lead, trail);
  }
  __GBK_MAP = map;
  return map;
}
function __encodeGbk(value) {
  var text = String(value == null ? "" : value);
  var map = __gbkMap();
  var bytes = [];
  for (var i = 0; i < text.length; i++) {
    var code = text.charCodeAt(i);
    if (code < 0x80) {
      bytes.push(code);
      continue;
    }
    var ch = text.charAt(i);
    var pair = map[ch];
    if (!pair) throw new Error("漫画源兼容层：GB2312 无法编码字符「" + ch + "」");
    bytes.push(pair[0], pair[1]);
  }
  var buffer = new ArrayBuffer(bytes.length);
  var view = new Uint8Array(buffer);
  for (var j = 0; j < bytes.length; j++) view[j] = bytes[j];
  return buffer;
}

// ── 摘要 / HMAC / AES（纯 JS，不依赖宿主 Node 能力）──────────────
function __toBytes(input) {
  if (input == null) return new Uint8Array(0);
  if (input instanceof Uint8Array) return input;
  if (input instanceof ArrayBuffer) return new Uint8Array(input);
  if (input && input.buffer instanceof ArrayBuffer) {
    return new Uint8Array(input.buffer, input.byteOffset || 0, input.byteLength || input.length || 0);
  }
  if (typeof input === "string") return new Uint8Array(__lunac_utf8(input));
  return new Uint8Array(input);
}
function __bytesToBuffer(bytes) {
  var copy = new Uint8Array(bytes.length);
  copy.set(bytes);
  return copy.buffer;
}
function __hex(bytes) {
  var out = "";
  for (var i = 0; i < bytes.length; i++) out += ("0" + bytes[i].toString(16)).slice(-2);
  return out;
}

var __MD5_K = [
  0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
  0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
  0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
  0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
  0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
  0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
  0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
  0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391
];
var __MD5_S = [
  7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
  5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
  4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
  6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21
];
function __md5(bytes) {
  var len = bytes.length;
  var total = (((len + 8) >> 6) + 1) * 64;
  var padded = new Uint8Array(total);
  padded.set(bytes);
  padded[len] = 0x80;
  var view = new DataView(padded.buffer);
  view.setUint32(total - 8, (len << 3) >>> 0, true);
  view.setUint32(total - 4, Math.floor(len / 536870912) >>> 0, true);
  var a0 = 0x67452301, b0 = 0xefcdab89, c0 = 0x98badcfe, d0 = 0x10325476;
  for (var off = 0; off < total; off += 64) {
    var m = new Array(16);
    for (var i = 0; i < 16; i++) m[i] = view.getUint32(off + i * 4, true);
    var a = a0, b = b0, c = c0, d = d0;
    for (var j = 0; j < 64; j++) {
      var f, g;
      if (j < 16) { f = (b & c) | (~b & d); g = j; }
      else if (j < 32) { f = (d & b) | (~d & c); g = (5 * j + 1) & 15; }
      else if (j < 48) { f = b ^ c ^ d; g = (3 * j + 5) & 15; }
      else { f = c ^ (b | ~d); g = (7 * j) & 15; }
      var temp = d;
      d = c;
      c = b;
      var sum = (a + f + __MD5_K[j] + m[g]) >>> 0;
      var shift = __MD5_S[j];
      b = (b + ((sum << shift) | (sum >>> (32 - shift)))) >>> 0;
      a = temp;
    }
    a0 = (a0 + a) >>> 0;
    b0 = (b0 + b) >>> 0;
    c0 = (c0 + c) >>> 0;
    d0 = (d0 + d) >>> 0;
  }
  var out = new ArrayBuffer(16);
  var ov = new DataView(out);
  ov.setUint32(0, a0, true);
  ov.setUint32(4, b0, true);
  ov.setUint32(8, c0, true);
  ov.setUint32(12, d0, true);
  return out;
}

var __SHA256_K = [
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2
];
function __rotr32(x, n) { return (x >>> n) | (x << (32 - n)); }
function __sha256(bytes) {
  var len = bytes.length;
  var total = (((len + 8) >> 6) + 1) * 64;
  var padded = new Uint8Array(total);
  padded.set(bytes);
  padded[len] = 0x80;
  var view = new DataView(padded.buffer);
  view.setUint32(total - 8, Math.floor(len / 536870912) >>> 0);
  view.setUint32(total - 4, (len << 3) >>> 0);
  var h0 = 0x6a09e667, h1 = 0xbb67ae85, h2 = 0x3c6ef372, h3 = 0xa54ff53a;
  var h4 = 0x510e527f, h5 = 0x9b05688c, h6 = 0x1f83d9ab, h7 = 0x5be0cd19;
  for (var off = 0; off < total; off += 64) {
    var w = new Array(64);
    for (var i = 0; i < 16; i++) w[i] = view.getUint32(off + i * 4);
    for (var i = 16; i < 64; i++) {
      var s0 = __rotr32(w[i - 15], 7) ^ __rotr32(w[i - 15], 18) ^ (w[i - 15] >>> 3);
      var s1 = __rotr32(w[i - 2], 17) ^ __rotr32(w[i - 2], 19) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }
    var a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, h = h7;
    for (var i = 0; i < 64; i++) {
      var S1 = __rotr32(e, 6) ^ __rotr32(e, 11) ^ __rotr32(e, 25);
      var ch = (e & f) ^ (~e & g);
      var t1 = (h + S1 + ch + __SHA256_K[i] + w[i]) >>> 0;
      var S0 = __rotr32(a, 2) ^ __rotr32(a, 13) ^ __rotr32(a, 22);
      var maj = (a & b) ^ (a & c) ^ (b & c);
      var t2 = (S0 + maj) >>> 0;
      h = g; g = f; f = e;
      e = (d + t1) >>> 0;
      d = c; c = b; b = a;
      a = (t1 + t2) >>> 0;
    }
    h0 = (h0 + a) >>> 0; h1 = (h1 + b) >>> 0; h2 = (h2 + c) >>> 0; h3 = (h3 + d) >>> 0;
    h4 = (h4 + e) >>> 0; h5 = (h5 + f) >>> 0; h6 = (h6 + g) >>> 0; h7 = (h7 + h) >>> 0;
  }
  var out = new ArrayBuffer(32);
  var ov = new DataView(out);
  var hs = [h0, h1, h2, h3, h4, h5, h6, h7];
  for (var i = 0; i < 8; i++) ov.setUint32(i * 4, hs[i]);
  return out;
}

var __SHA512_K_HEX = [
  "428a2f98d728ae22", "7137449123ef65cd", "b5c0fbcfec4d3b2f", "e9b5dba58189dbbc",
  "3956c25bf348b538", "59f111f1b605d019", "923f82a4af194f9b", "ab1c5ed5da6d8118",
  "d807aa98a3030242", "12835b0145706fbe", "243185be4ee4b28c", "550c7dc3d5ffb4e2",
  "72be5d74f27b896f", "80deb1fe3b1696b1", "9bdc06a725c71235", "c19bf174cf692694",
  "e49b69c19ef14ad2", "efbe4786384f25e3", "0fc19dc68b8cd5b5", "240ca1cc77ac9c65",
  "2de92c6f592b0275", "4a7484aa6ea6e483", "5cb0a9dcbd41fbd4", "76f988da831153b5",
  "983e5152ee66dfab", "a831c66d2db43210", "b00327c898fb213f", "bf597fc7beef0ee4",
  "c6e00bf33da88fc2", "d5a79147930aa725", "06ca6351e003826f", "142929670a0e6e70",
  "27b70a8546d22ffc", "2e1b21385c26c926", "4d2c6dfc5ac42aed", "53380d139d95b3df",
  "650a73548baf63de", "766a0abb3c77b2a8", "81c2c92e47edaee6", "92722c851482353b",
  "a2bfe8a14cf10364", "a81a664bbc423001", "c24b8b70d0f89791", "c76c51a30654be30",
  "d192e819d6ef5218", "d69906245565a910", "f40e35855771202a", "106aa07032bbd1b8",
  "19a4c116b8d2d0c8", "1e376c085141ab53", "2748774cdf8eeb99", "34b0bcb5e19b48a8",
  "391c0cb3c5c95a63", "4ed8aa4ae3418acb", "5b9cca4f7763e373", "682e6ff3d6b2b8a3",
  "748f82ee5defb2fc", "78a5636f43172f60", "84c87814a1f0ab72", "8cc702081a6439ec",
  "90befffa23631e28", "a4506cebde82bde9", "bef9a3f7b2c67915", "c67178f2e372532b",
  "ca273eceea26619c", "d186b8c721c0c207", "eada7dd6cde0eb1e", "f57d4f7fee6ed178",
  "06f067aa72176fba", "0a637dc5a2c898a6", "113f9804bef90dae", "1b710b35131c471b",
  "28db77f523047d84", "32caab7b40c72493", "3c9ebe0a15c9bebc", "431d67c49c100d4c",
  "4cc5d4becb3e42b6", "597f299cfc657e2a", "5fcb6fab3ad6faec", "6c44198c4a475817"
];
var __SHA512_H_HEX = [
  "6a09e667f3bcc908", "bb67ae8584caa73b", "3c6ef372fe94f82b", "a54ff53a5f1d36f1",
  "510e527fade682d1", "9b05688c2b3e6c1f", "1f83d9abfb41bd6b", "5be0cd19137e2179"
];
function __sha512(bytes) {
  if (typeof BigInt !== "function") {
    throw new Error("漫画源兼容层：当前运行时没有 BigInt，无法执行 sha512");
  }
  var zero = BigInt(0), one = BigInt(1), m64 = BigInt("0xffffffffffffffff");
  var k = [], hs = [];
  for (var i = 0; i < 80; i++) k.push(BigInt("0x" + __SHA512_K_HEX[i]));
  for (var i = 0; i < 8; i++) hs.push(BigInt("0x" + __SHA512_H_HEX[i]));
  function rotr(x, n) { return ((x >> BigInt(n)) | (x << BigInt(64 - n))) & m64; }
  var len = bytes.length;
  var total = (Math.floor((len + 16) / 128) + 1) * 128;
  var padded = new Uint8Array(total);
  padded.set(bytes);
  padded[len] = 0x80;
  var view = new DataView(padded.buffer);
  view.setUint32(total - 8, Math.floor(len / 536870912) >>> 0);
  view.setUint32(total - 4, (len << 3) >>> 0);
  function read64(offset) {
    var hi = 0, lo = 0;
    for (var j = 0; j < 4; j++) hi = hi * 256 + padded[offset + j];
    for (var j = 4; j < 8; j++) lo = lo * 256 + padded[offset + j];
    return (BigInt(hi) << BigInt(32)) | BigInt(lo);
  }
  for (var off = 0; off < total; off += 128) {
    var w = [];
    for (var i = 0; i < 16; i++) w.push(read64(off + i * 8));
    for (var i = 16; i < 80; i++) {
      var x = w[i - 15], y = w[i - 2];
      var s0 = rotr(x, 1) ^ rotr(x, 8) ^ (x >> BigInt(7));
      var s1 = rotr(y, 19) ^ rotr(y, 61) ^ (y >> BigInt(6));
      w.push((w[i - 16] + s0 + w[i - 7] + s1) & m64);
    }
    var a = hs[0], b = hs[1], c = hs[2], d = hs[3];
    var e = hs[4], f = hs[5], g = hs[6], h = hs[7];
    for (var i = 0; i < 80; i++) {
      var S1 = rotr(e, 14) ^ rotr(e, 18) ^ rotr(e, 41);
      var ch = (e & f) ^ ((~e & m64) & g);
      var t1 = (h + S1 + ch + k[i] + w[i]) & m64;
      var S0 = rotr(a, 28) ^ rotr(a, 34) ^ rotr(a, 39);
      var maj = (a & b) ^ (a & c) ^ (b & c);
      var t2 = (S0 + maj) & m64;
      h = g; g = f; f = e;
      e = (d + t1) & m64;
      d = c; c = b; b = a;
      a = (t1 + t2) & m64;
    }
    hs[0] = (hs[0] + a) & m64; hs[1] = (hs[1] + b) & m64;
    hs[2] = (hs[2] + c) & m64; hs[3] = (hs[3] + d) & m64;
    hs[4] = (hs[4] + e) & m64; hs[5] = (hs[5] + f) & m64;
    hs[6] = (hs[6] + g) & m64; hs[7] = (hs[7] + h) & m64;
  }
  var out = new Uint8Array(64);
  for (var i = 0; i < 8; i++) {
    var word = hs[i];
    for (var j = 7; j >= 0; j--) {
      out[i * 8 + j] = Number(word & BigInt(255));
      word = word >> BigInt(8);
    }
  }
  return out.buffer;
}

function __sha1(bytes) {
  var len = bytes.length;
  var total = (((len + 8) >> 6) + 1) * 64;
  var padded = new Uint8Array(total);
  padded.set(bytes);
  padded[len] = 0x80;
  var view = new DataView(padded.buffer);
  view.setUint32(total - 8, Math.floor(len / 536870912) >>> 0);
  view.setUint32(total - 4, (len << 3) >>> 0);
  var h0 = 0x67452301, h1 = 0xefcdab89, h2 = 0x98badcfe, h3 = 0x10325476, h4 = 0xc3d2e1f0;
  var w = new Array(80);
  for (var off = 0; off < total; off += 64) {
    for (var i = 0; i < 16; i++) w[i] = view.getUint32(off + i * 4);
    for (var i = 16; i < 80; i++) {
      var x = w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16];
      w[i] = ((x << 1) | (x >>> 31)) >>> 0;
    }
    var a = h0, b = h1, c = h2, d = h3, e = h4;
    for (var i = 0; i < 80; i++) {
      var f, k;
      if (i < 20) { f = (b & c) | (~b & d); k = 0x5a827999; }
      else if (i < 40) { f = b ^ c ^ d; k = 0x6ed9eba1; }
      else if (i < 60) { f = (b & c) | (b & d) | (c & d); k = 0x8f1bbcdc; }
      else { f = b ^ c ^ d; k = 0xca62c1d6; }
      var temp = ((((a << 5) | (a >>> 27)) >>> 0) + f + e + k + w[i]) >>> 0;
      e = d;
      d = c;
      c = ((b << 30) | (b >>> 2)) >>> 0;
      b = a;
      a = temp;
    }
    h0 = (h0 + a) >>> 0;
    h1 = (h1 + b) >>> 0;
    h2 = (h2 + c) >>> 0;
    h3 = (h3 + d) >>> 0;
    h4 = (h4 + e) >>> 0;
  }
  var out = new ArrayBuffer(20);
  var ov = new DataView(out);
  ov.setUint32(0, h0);
  ov.setUint32(4, h1);
  ov.setUint32(8, h2);
  ov.setUint32(12, h3);
  ov.setUint32(16, h4);
  return out;
}

function __hashBytes(name, bytes) {
  var algo = String(name || "sha256").toLowerCase();
  if (algo === "md5") return __md5(bytes);
  if (algo === "sha1") return __sha1(bytes);
  if (algo === "sha256") return __sha256(bytes);
  if (algo === "sha512") return __sha512(bytes);
  throw new Error("漫画源兼容层暂未实现 Convert." + algo);
}
function __hmacBytes(name, keyInput, dataInput) {
  var algo = String(name || "sha256").toLowerCase();
  var block = algo === "sha512" ? 128 : 64;
  var key = __toBytes(keyInput);
  if (key.length > block) key = new Uint8Array(__hashBytes(algo, key));
  var ipad = new Uint8Array(block);
  var opad = new Uint8Array(block);
  for (var i = 0; i < block; i++) {
    var value = i < key.length ? key[i] : 0;
    ipad[i] = value ^ 0x36;
    opad[i] = value ^ 0x5c;
  }
  var data = __toBytes(dataInput);
  var innerData = new Uint8Array(block + data.length);
  innerData.set(ipad);
  innerData.set(data, block);
  var inner = new Uint8Array(__hashBytes(algo, innerData));
  var outerData = new Uint8Array(block + inner.length);
  outerData.set(opad);
  outerData.set(inner, block);
  return __hashBytes(algo, outerData);
}

// AES 解密（CBC / ECB）：S 盒按 GF(2^8) 现算，省掉两份大表。
var __AES_SBOX = null;
var __AES_INV_SBOX = null;
function __aesMul(a, b) {
  var p = 0;
  for (var i = 0; i < 8; i++) {
    if (b & 1) p ^= a;
    var hi = a & 0x80;
    a = (a << 1) & 0xff;
    if (hi) a ^= 0x1b;
    b >>= 1;
  }
  return p;
}
function __aesPow(x, n) {
  var result = 1;
  while (n > 0) {
    if (n & 1) result = __aesMul(result, x);
    x = __aesMul(x, x);
    n >>= 1;
  }
  return result;
}
function __aesTables() {
  if (__AES_SBOX) return;
  var sbox = new Array(256);
  var inv = new Array(256);
  for (var i = 0; i < 256; i++) {
    // GF(2^8) 乘法逆元：x^254（0 仍为 0）。
    var x = i === 0 ? 0 : __aesPow(i, 254);
    var s = x;
    for (var n = 1; n <= 4; n++) {
      s ^= ((x << n) | (x >> (8 - n))) & 0xff;
    }
    s ^= 0x63;
    sbox[i] = s & 0xff;
    inv[s & 0xff] = i;
  }
  __AES_SBOX = sbox;
  __AES_INV_SBOX = inv;
}
function __aesExpand(keyBytes) {
  __aesTables();
  var nk = keyBytes.length / 4;
  if (nk !== 4 && nk !== 6 && nk !== 8) {
    throw new Error("漫画源兼容层：AES 密钥长度必须是 16 / 24 / 32 字节");
  }
  var nr = nk + 6;
  var words = [];
  var i, j;
  for (i = 0; i < nk; i++) {
    words.push([keyBytes[i * 4], keyBytes[i * 4 + 1], keyBytes[i * 4 + 2], keyBytes[i * 4 + 3]]);
  }
  var rcon = 1;
  for (i = nk; i < 4 * (nr + 1); i++) {
    var temp = words[i - 1].slice();
    if (i % nk === 0) {
      temp = [temp[1], temp[2], temp[3], temp[0]];
      for (j = 0; j < 4; j++) temp[j] = __AES_SBOX[temp[j]];
      temp[0] ^= rcon;
      rcon = __aesMul(rcon, 2);
    } else if (nk > 6 && i % nk === 4) {
      for (j = 0; j < 4; j++) temp[j] = __AES_SBOX[temp[j]];
    }
    var prev = words[i - nk];
    words.push([prev[0] ^ temp[0], prev[1] ^ temp[1], prev[2] ^ temp[2], prev[3] ^ temp[3]]);
  }
  return { words: words, rounds: nr };
}
function __aesAddRoundKey(state, words, round) {
  for (var i = 0; i < 16; i++) {
    state[i] ^= words[round * 4 + (i >> 2)][i & 3];
  }
}
function __aesInvShiftRows(state) {
  var out = state.slice();
  for (var row = 1; row < 4; row++) {
    for (var col = 0; col < 4; col++) {
      out[row + 4 * ((col + row) % 4)] = state[row + 4 * col];
    }
  }
  return out;
}
function __aesInvMixColumns(state) {
  var out = state.slice();
  for (var col = 0; col < 4; col++) {
    var i = col * 4;
    var a0 = state[i], a1 = state[i + 1], a2 = state[i + 2], a3 = state[i + 3];
    out[i] = __aesMul(a0, 14) ^ __aesMul(a1, 11) ^ __aesMul(a2, 13) ^ __aesMul(a3, 9);
    out[i + 1] = __aesMul(a0, 9) ^ __aesMul(a1, 14) ^ __aesMul(a2, 11) ^ __aesMul(a3, 13);
    out[i + 2] = __aesMul(a0, 13) ^ __aesMul(a1, 9) ^ __aesMul(a2, 14) ^ __aesMul(a3, 11);
    out[i + 3] = __aesMul(a0, 11) ^ __aesMul(a1, 13) ^ __aesMul(a2, 9) ^ __aesMul(a3, 14);
  }
  return out;
}
function __aesDecryptBlock(input, expanded) {
  var state = [];
  var i;
  for (i = 0; i < 16; i++) state.push(input[i]);
  __aesAddRoundKey(state, expanded.words, expanded.rounds);
  for (var round = expanded.rounds - 1; round >= 1; round--) {
    state = __aesInvShiftRows(state);
    for (i = 0; i < 16; i++) state[i] = __AES_INV_SBOX[state[i]];
    __aesAddRoundKey(state, expanded.words, round);
    state = __aesInvMixColumns(state);
  }
  state = __aesInvShiftRows(state);
  for (i = 0; i < 16; i++) state[i] = __AES_INV_SBOX[state[i]];
  __aesAddRoundKey(state, expanded.words, 0);
  return state;
}
function __aesCbcDecrypt(dataInput, keyInput, ivInput) {
  var data = __toBytes(dataInput);
  var key = __toBytes(keyInput);
  var iv = __toBytes(ivInput);
  if (iv.length !== 16) throw new Error("漫画源兼容层：AES-CBC 的 iv 必须是 16 字节");
  if (data.length % 16 !== 0) throw new Error("漫画源兼容层：AES-CBC 密文长度必须是 16 的倍数");
  var expanded = __aesExpand(key);
  var out = new Uint8Array(data.length);
  var prev = iv;
  for (var off = 0; off < data.length; off += 16) {
    var block = data.subarray(off, off + 16);
    var plain = __aesDecryptBlock(block, expanded);
    for (var i = 0; i < 16; i++) out[off + i] = plain[i] ^ prev[i];
    prev = block;
  }
  return out.buffer;
}
function __aesEcbDecrypt(dataInput, keyInput) {
  var data = __toBytes(dataInput);
  var key = __toBytes(keyInput);
  if (data.length % 16 !== 0) throw new Error("漫画源兼容层：AES-ECB 密文长度必须是 16 的倍数");
  var expanded = __aesExpand(key);
  var out = new Uint8Array(data.length);
  for (var off = 0; off < data.length; off += 16) {
    var plain = __aesDecryptBlock(data.subarray(off, off + 16), expanded);
    for (var i = 0; i < 16; i++) out[off + i] = plain[i];
  }
  return out.buffer;
}
function __aesShiftRows(state) {
  var out = state.slice();
  for (var row = 1; row < 4; row++) {
    for (var col = 0; col < 4; col++) {
      out[row + 4 * col] = state[row + 4 * ((col + row) % 4)];
    }
  }
  return out;
}
function __aesMixColumns(state) {
  var out = state.slice();
  for (var col = 0; col < 4; col++) {
    var i = col * 4;
    var a0 = state[i], a1 = state[i + 1], a2 = state[i + 2], a3 = state[i + 3];
    out[i] = __aesMul(a0, 2) ^ __aesMul(a1, 3) ^ a2 ^ a3;
    out[i + 1] = a0 ^ __aesMul(a1, 2) ^ __aesMul(a2, 3) ^ a3;
    out[i + 2] = a0 ^ a1 ^ __aesMul(a2, 2) ^ __aesMul(a3, 3);
    out[i + 3] = __aesMul(a0, 3) ^ a1 ^ a2 ^ __aesMul(a3, 2);
  }
  return out;
}
function __aesEncryptBlock(input, expanded) {
  var state = [];
  var i;
  for (i = 0; i < 16; i++) state.push(input[i]);
  __aesTables();
  __aesAddRoundKey(state, expanded.words, 0);
  for (var round = 1; round < expanded.rounds; round++) {
    for (i = 0; i < 16; i++) state[i] = __AES_SBOX[state[i]];
    state = __aesShiftRows(state);
    state = __aesMixColumns(state);
    __aesAddRoundKey(state, expanded.words, round);
  }
  for (i = 0; i < 16; i++) state[i] = __AES_SBOX[state[i]];
  state = __aesShiftRows(state);
  __aesAddRoundKey(state, expanded.words, expanded.rounds);
  return state;
}
function __aesCbcEncrypt(dataInput, keyInput, ivInput) {
  var data = __toBytes(dataInput);
  var key = __toBytes(keyInput);
  var iv = __toBytes(ivInput);
  if (iv.length !== 16) throw new Error("漫画源兼容层：AES-CBC 的 iv 必须是 16 字节");
  if (data.length % 16 !== 0) throw new Error("漫画源兼容层：AES-CBC 明文长度必须是 16 的倍数");
  var expanded = __aesExpand(key);
  var out = new Uint8Array(data.length);
  var prev = iv;
  for (var off = 0; off < data.length; off += 16) {
    var block = new Uint8Array(16);
    for (var i = 0; i < 16; i++) block[i] = data[off + i] ^ prev[i];
    var cipher = __aesEncryptBlock(block, expanded);
    for (var i = 0; i < 16; i++) out[off + i] = cipher[i];
    prev = cipher;
  }
  return out.buffer;
}
function __aesEcbEncrypt(dataInput, keyInput) {
  var data = __toBytes(dataInput);
  var key = __toBytes(keyInput);
  if (data.length % 16 !== 0) throw new Error("漫画源兼容层：AES-ECB 明文长度必须是 16 的倍数");
  var expanded = __aesExpand(key);
  var out = new Uint8Array(data.length);
  for (var off = 0; off < data.length; off += 16) {
    var cipher = __aesEncryptBlock(data.subarray(off, off + 16), expanded);
    for (var i = 0; i < 16; i++) out[off + i] = cipher[i];
  }
  return out.buffer;
}

var Convert = {
  encodeUtf8: function (value) { return __lunac_utf8(value); },
  decodeUtf8: function (buffer) { return __lunac_utf8_decode(buffer); },
  encodeBase64: function (buffer) { return __lunac_b64encode(buffer); },
  decodeBase64: function (value) { return __lunac_b64decode(value); },
  encodeGbk: function (value) { return __encodeGbk(value); },
  hexEncode: function (buffer) {
    var view = new Uint8Array(buffer);
    var out = "";
    for (var i = 0; i < view.length; i++) out += ("0" + view[i].toString(16)).slice(-2);
    return out;
  },
  md5: function (buffer) { return __md5(__toBytes(buffer)); },
  sha1: function (buffer) { return __sha1(__toBytes(buffer)); },
  sha256: function (buffer) { return __sha256(__toBytes(buffer)); },
  sha512: function (buffer) { return __sha512(__toBytes(buffer)); },
  hmac: function (key, data, algorithm) {
    return __hmacBytes(algorithm || "sha256", key, data);
  },
  hmacString: function (key, data, algorithm) {
    return __hex(new Uint8Array(__hmacBytes(algorithm || "sha256", key, data)));
  },
  encryptAesEcb: function (data, key) { return __aesEcbEncrypt(data, key); },
  decryptAesEcb: function (data, key) { return __aesEcbDecrypt(data, key); },
  encryptAesCbc: function (data, key, iv) { return __aesCbcEncrypt(data, key, iv); },
  decryptAesCbc: function (data, key, iv) { return __aesCbcDecrypt(data, key, iv); },
  encryptAesCfb: function () { throw new Error("漫画源兼容层暂未实现 AES 加密"); },
  decryptAesCfb: function () { throw new Error("漫画源兼容层暂未实现 AES-CFB"); },
  encryptAesOfb: function () { throw new Error("漫画源兼容层暂未实现 AES 加密"); },
  decryptAesOfb: function () { throw new Error("漫画源兼容层暂未实现 AES-OFB"); },
};

var Crypto = {
  md5: Convert.md5,
  sha1: Convert.sha1,
  sha256: Convert.sha256,
  sha512: Convert.sha512,
  hmac: Convert.hmac,
  hmacString: Convert.hmacString,
};

var UI = {
  toast: function () {},
  showDialog: function () { return Promise.resolve(null); },
  showLoading: function () {},
  hideLoading: function () {},
};
`.replace("__GB2312_TABLE__", GB2312_CHARS);

// ── 调用入口（拼在上游脚本之后）────────────────────────────────
export const COMIC_RUNNER = String.raw`
function __lunac_toChapters(value) {
  var out = [];
  if (!value) return out;
  if (typeof value.entries === "function" && typeof value.get === "function") {
    var iterator = value.entries();
    var step = iterator.next();
    while (!step.done) {
      out.push({ id: String(step.value[0]), title: String(step.value[1] == null ? step.value[0] : step.value[1]) });
      step = iterator.next();
    }
    return out;
  }
  if (Array.isArray(value)) {
    for (var i = 0; i < value.length; i++) {
      var item = value[i];
      if (item && typeof item === "object") {
        out.push({ id: String(item.id == null ? i : item.id), title: String(item.title || item.name || item.id || i) });
      } else {
        out.push({ id: String(i), title: String(item) });
      }
    }
    return out;
  }
  if (typeof value === "object") {
    for (var key in value) {
      if (Object.prototype.hasOwnProperty.call(value, key)) {
        out.push({ id: String(key), title: String(value[key] == null ? key : value[key]) });
      }
    }
  }
  return out;
}

// 上游 tags 有两种写法：字符串数组，或「分组 → 标签列表」对象
// （百合会写的是 {作者:[…], 更新:[…], 标签:[…]}）。对象形态拍平成「分组：值」。
function __lunac_tags(value) {
  if (Array.isArray(value)) return value.length ? value : undefined;
  if (!value || typeof value !== "object") return undefined;
  var out = [];
  for (var group in value) {
    if (!Object.prototype.hasOwnProperty.call(value, group)) continue;
    var items = Array.isArray(value[group]) ? value[group] : [value[group]];
    var values = [];
    for (var i = 0; i < items.length; i++) {
      if (items[i] == null) continue;
      var item = String(items[i]).trim();
      if (item) values.push(item);
    }
    if (values.length) out.push(group + "：" + values.join(" / "));
  }
  return out.length ? out : undefined;
}
// 图片请求头：章节级（loadEp 返回的 headers）在前，单张配置（onImageLoad.headers）覆盖它。
function __lunac_merge_headers(base, extra) {
  var out = {};
  var groups = [base, extra];
  for (var g = 0; g < groups.length; g++) {
    var group = groups[g];
    if (!group || typeof group !== "object") continue;
    for (var key in group) {
      if (!Object.prototype.hasOwnProperty.call(group, key)) continue;
      var value = group[key];
      if (value === undefined || value === null) continue;
      out[key] = typeof value === "object" ? JSON.stringify(value) : String(value);
    }
  }
  return out;
}

// 上游 comic.onImageLoad(url, comicId, epId) / comic.onThumbnailLoad(url)：
// 返回 { url, headers, onResponse }。章节页只认 onImageLoad，封面只认 onThumbnailLoad ——
// 两者语义不同，混用会把章节图按缩略图规则再处理一次。
function __lunac_image_config(source, url, workId, chapterId) {
  var comic = (source && source.comic) || {};
  var fn = chapterId ? comic.onImageLoad : comic.onThumbnailLoad;
  if (typeof fn !== "function") return null;
  var config;
  try {
    config = chapterId ? fn.call(comic, url, workId, chapterId) : fn.call(comic, url);
  } catch (error) {
    return null;
  }
  return config && typeof config === "object" ? config : null;
}

function __lunac_image_bytes(value) {
  if (value == null) return null;
  if (
    typeof ArrayBuffer !== "undefined" &&
    typeof ArrayBuffer.isView === "function" &&
    ArrayBuffer.isView(value)
  ) {
    return value.buffer.slice(value.byteOffset || 0, (value.byteOffset || 0) + value.byteLength);
  }
  return value;
}

function __lunac_toWork(comic) {
  return {
    id: String(comic.id == null ? "" : comic.id),
    title: String(comic.title == null ? comic.id : comic.title),
    author: comic.subTitle ? String(comic.subTitle) : undefined,
    cover: comic.cover ? String(comic.cover) : undefined,
    description: comic.description ? String(comic.description) : undefined,
    tags: __lunac_tags(comic.tags),
    status: comic.status === "completed" || comic.status === "ongoing" ? comic.status : undefined,
  };
}

async function __lunac_main() {
  var className = args.className;
  var SourceClass = null;
  try {
    SourceClass = eval(className);
  } catch (error) {
    SourceClass = globalThis[className];
  }
  if (typeof SourceClass !== "function") {
    throw new Error("未找到漫画源类：" + className);
  }
  var source = new SourceClass();
  source.__overrides = (args && args.settings) || {};
  if (typeof source.init === "function") {
    try { source.init(); } catch (error) { /* 初始化失败不阻断读取 */ }
  }

  if (args.method === "search") {
    if (!source.search || typeof source.search.load !== "function") {
      throw new Error("该漫画源没有实现 search.load");
    }
    var page = await source.search.load(args.query, [], args.page || 1);
    var comics = (page && page.comics) || [];
    return {
      items: comics.map(__lunac_toWork),
      maxPage: page && page.maxPage != null ? page.maxPage : undefined,
    };
  }

  if (args.method === "explore") {
    var areas = source.explore;
    if (!Array.isArray(areas) || areas.length === 0) {
      throw new Error("该漫画源没有实现 explore");
    }
    var area = null;
    for (var ai = 0; ai < areas.length; ai++) {
      if (!args.section || String(areas[ai].title || "") === String(args.section)) {
        area = areas[ai];
        break;
      }
    }
    if (!area) area = areas[0];
    if (typeof area.load !== "function") {
      throw new Error("漫画源 explore 条目缺少 load");
    }
    // singlePageWithMultiPart 的 load() 返回 { 分区: [comic] }；
    // multiPageComicList 返回 { comics, maxPage }。
    // 部分上游用普通函数写 load，需要把 this 绑回来源实例。
    var loaded = await area.load.call(source, args.page || 1);
    var exploreComics = [];
    if (Array.isArray(loaded)) {
      exploreComics = loaded;
    } else if (loaded && Array.isArray(loaded.comics)) {
      exploreComics = loaded.comics;
    } else if (loaded && typeof loaded === "object") {
      for (var part in loaded) {
        if (Object.prototype.hasOwnProperty.call(loaded, part) && Array.isArray(loaded[part])) {
          exploreComics = exploreComics.concat(loaded[part]);
        }
      }
    }
    return {
      items: exploreComics.map(__lunac_toWork),
      maxPage: loaded && loaded.maxPage != null ? loaded.maxPage : undefined,
    };
  }

  if (!source.comic) throw new Error("该漫画源没有实现 comic");

  if (args.method === "detail") {
    if (typeof source.comic.loadInfo !== "function") {
      throw new Error("该漫画源没有实现 comic.loadInfo");
    }
    var info = await source.comic.loadInfo(args.workId);
    var detailWork = __lunac_toWork(info || {});
    if (!detailWork.id) detailWork.id = String(args.workId);
    return { work: detailWork };
  }

  if (args.method === "chapters") {
    if (typeof source.comic.loadInfo !== "function") {
      throw new Error("该漫画源没有实现 comic.loadInfo");
    }
    var details = await source.comic.loadInfo(args.workId);
    return { chapters: __lunac_toChapters(details && details.chapters) };
  }

  if (args.method === "pages") {
    if (typeof source.comic.loadEp !== "function") {
      throw new Error("该漫画源没有实现 comic.loadEp");
    }
    var episode = await source.comic.loadEp(args.workId, args.chapterId);
    var images = (episode && episode.images) || [];
    var episodeHeaders = (episode && episode.headers) || {};
    var pages = [];
    for (var pi = 0; pi < images.length; pi++) {
      var image = images[pi];
      var imageUrl = typeof image === "string"
        ? image
        : String((image && (image.url || image.src)) || "");
      if (!imageUrl) continue;
      // onImageLoad 可能改写地址（CDN 换域）、补 Referer / UA，或用 onResponse 解密字节。
      var imageConfig = __lunac_image_config(source, imageUrl, args.workId, args.chapterId);
      pages.push({
        url: imageConfig && imageConfig.url ? String(imageConfig.url) : imageUrl,
        headers: __lunac_merge_headers(episodeHeaders, imageConfig && imageConfig.headers),
        transform: !!(imageConfig && typeof imageConfig.onResponse === "function"),
      });
    }
    return { pages: pages };
  }

  // 图片后处理：源声明了 onImageLoad / onThumbnailLoad 的 onResponse（如 AES 加密图源），
  // 插件只能拿到密文 —— 由沙箱取字节、按源自己的规则解成明文再回传（base64）。
  if (args.method === "image") {
    var config = __lunac_image_config(source, args.url, args.workId, args.chapterId);
    if (!config || typeof config.onResponse !== "function") return { data: null };
    var target = config.url ? String(config.url) : String(args.url);
    var headers = __lunac_merge_headers(args.headers, config.headers);
    var fetched = Network.fetchBytes("GET", target, headers, null);
    if (!fetched || fetched.status < 200 || fetched.status >= 300 || !fetched.body) {
      throw new Error("图片下载失败：HTTP " + ((fetched && fetched.status) || 0));
    }
    var plain = __lunac_image_bytes(await config.onResponse(fetched.body));
    if (!plain) return { data: null };
    return { data: __lunac_b64encode(plain) };
  }

  throw new Error("漫画源兼容层不支持方法：" + args.method);
}

__lunac_main().then(function (result) {
  if (__lunac_data_dirty && result && typeof result === "object") {
    result.__lunac_data = __lunac_stored_data === undefined ? null : __lunac_stored_data;
  }
  return result;
})
`;

// ── 视频源（type 101）兼容层 ────────────────────────────────────
// 上游扩展是「全局函数 + req + cheerio」形态；这里按接口翻译，不复制上游代码。
// 复用 COMIC_PRELUDE 注入的 host.request / HTML 解析器 / UTF-8 工具。
export const VIDEO_PRELUDE = String.raw`
var __uz_env = {};
var __uz_storage = {};

function ProData() {
  this.error = "";
  this.data = null;
  this.headers = {};
  this.code = 0;
  this.redirects = [];
}
ProData.prototype.ok = function () { return this.code === 200; };

var ReqResponseType = { json: "json", arraybuffer: "arraybuffer", bytes: "bytes", plain: "plain", stream: "stream" };
var ReqAddressType = { any: "any", ipv4: "ipv4", ipv6: "ipv6" };

// ── uzVideo 数据类 ───────────────────────────────────────────────
// 线上扩展不拼对象字面量，而是 new RepVideoList() 后逐字段赋值；缺一个就是
// 整源 ReferenceError。字段清单对齐上游 core/uzVideo.js 的公开定义，只有默认值。
function FilterLabel() { this.name = ""; this.id = ""; this.key = ""; }
function FilterTitle() { this.name = ""; this.list = []; }
function VideoClass() { this.type_id = ""; this.type_name = ""; this.hasSubclass = false; }
function VideoSubclass() { this.class = []; this.filter = []; }
function VideoDetail() {
  this.vod_id = "";
  this.vod_name = "";
  this.vod_play_from = "";
  this.vod_play_url = "";
  this.vod_pic = "";
  this.type_name = "";
  this.vod_remarks = "";
  this.topRightRemarks = "";
  this.vod_douban_score = "";
  this.vod_lang = "";
  this.vod_year = "";
  this.vod_actor = "";
  this.vod_director = "";
  this.vod_content = "";
  this.vod_area = "";
  this.goSearch = false;
  this.panUrls = [];
}
function VodDanMu() { this.content = ""; this.time = 0; }
function RepVideoClassList() { this.data = []; this.error = ""; }
function RepVideoSubclassList() { this.data = new VideoSubclass(); this.error = ""; }
function RepVideoList() { this.data = []; this.error = ""; this.total = 0; }
function RepVideoDetail() { this.data = null; this.error = ""; }
function RepVideoPlayUrl() {
  this.data = "";
  this.urls = [];
  this.headers = undefined;
  this.sniffer = {};
  this.danMu = [];
  this.error = "";
}
function UZArgs() { this.url = ""; this.page = 1; this.searchWord = ""; this.flag = ""; }
function UZSubclassVideoListArgs() {
  UZArgs.call(this);
  this.mainClassId = "";
  this.subclassId = "";
  this.filter = [];
}
UZSubclassVideoListArgs.prototype = Object.create(UZArgs.prototype);
UZSubclassVideoListArgs.prototype.constructor = UZSubclassVideoListArgs;

// 上游脚本会 console.log 调试；QuickJS 没有 console，缺了同样是 ReferenceError。
var console = {
  log: function () {},
  info: function () {},
  warn: function () {},
  error: function () {},
  debug: function () {},
};

function __uz_query(url, params) {
  if (!params) return url;
  var pairs = [];
  for (var key in params) {
    if (!Object.prototype.hasOwnProperty.call(params, key)) continue;
    var value = params[key];
    if (value === undefined || value === null) continue;
    pairs.push(encodeURIComponent(key) + "=" + encodeURIComponent(String(value)));
  }
  var query = pairs.join("&");
  if (!query) return url;
  return url + (url.indexOf("?") >= 0 ? "&" : "?") + query;
}

async function req(url, options) {
  var pro = new ProData();
  options = options || {};
  try {
    var finalUrl = __uz_query(String(url), options.queryParameters);
    var method = String(options.method || "GET").toUpperCase();
    var headers = {};
    if (options.headers) {
      for (var key in options.headers) {
        if (Object.prototype.hasOwnProperty.call(options.headers, key)) headers[key] = options.headers[key];
      }
    }
    var body;
    if (options.data !== undefined && options.data !== null) {
      if (options.isBinary) {
        body = __lunac_utf8_decode(new Uint8Array(options.data).buffer);
      } else if (typeof options.data === "string") {
        body = options.data;
      } else {
        body = JSON.stringify(options.data);
        var hasType = false;
        for (var hk in headers) {
          if (String(hk).toLowerCase() === "content-type") hasType = true;
        }
        if (!hasType) headers["Content-Type"] = "application/json";
      }
    }
    var response = __lunac_request(method, finalUrl, headers, body);
    pro.code = response.status;
    pro.headers = response.headers || {};
    if (response.url && response.url !== finalUrl) pro.redirects = [{ url: response.url }];
    var contentType = "";
    for (var rh in pro.headers) {
      if (String(rh).toLowerCase() === "content-type") contentType = String(pro.headers[rh]).toLowerCase();
    }
    var type = options.responseType;
    if (!type) {
      // 与上游 uzUtils.req 一致：application/json 直接解成对象，text/* 保持字符串，
      // 其余类型先试 JSON、发不出来再按字符串给（MacCMS 采集站常回 text/html）。
      if (contentType.indexOf("json") >= 0) type = "json";
      else if (contentType.indexOf("octet-stream") >= 0) type = "bytes";
      else if (contentType.indexOf("text/") >= 0) type = "plain";
      else type = "auto";
    }
    if (type === "json" || type === "auto") {
      try { pro.data = JSON.parse(response.body); } catch (error) { pro.data = response.body; }
    } else if (type === "arraybuffer" || type === "bytes") {
      pro.data = __lunac_utf8(response.body);
    } else {
      pro.data = response.body;
    }
  } catch (error) {
    pro.error = String((error && error.message) || error);
    pro.code = 500;
  }
  return pro;
}

var UZUtils = {
  debugLog: function () {},
  getHostFromURL: function (url) {
    var index = String(url).indexOf("://");
    if (index < 0) return null;
    var start = index + 3;
    var end = String(url).indexOf("/", start);
    return String(url).slice(0, end < 0 ? String(url).length : end);
  },
  removeTrailingSlash: function (value) {
    var text = String(value);
    return text.charAt(text.length - 1) === "/" ? text.slice(0, -1) : text;
  },
  getStrByRegexDefault: function (pattern, value) {
    var match = pattern.exec(String(value));
    return match && match.length > 1 ? match[1] : String(value);
  },
  lcs: function (a, b) {
    var s1 = String(a), s2 = String(b);
    var m = s1.length, n = s2.length;
    var dp = [];
    for (var i = 0; i <= m; i++) { dp.push([]); for (var j = 0; j <= n; j++) dp[i].push(0); }
    var best = 0;
    for (var x = 1; x <= m; x++) {
      for (var y = 1; y <= n; y++) {
        if (s1.charAt(x - 1) === s2.charAt(y - 1)) {
          dp[x][y] = dp[x - 1][y - 1] + 1;
          if (dp[x][y] > best) best = dp[x][y];
        }
      }
    }
    return best;
  },
  findIndex: function (list, element) {
    for (var i = 0; i < (list || []).length; i++) if (list[i] === element) return i;
    return -1;
  },
  findIgnoreCase: function (dict, key) {
    var target = String(key).toLowerCase();
    for (var name in dict) {
      if (Object.prototype.hasOwnProperty.call(dict, name) && name.toLowerCase() === target) return dict[name];
    }
    return undefined;
  },
  findValueIgnoreCaseInDict: function (dict, key) {
    var target = String(key).toLowerCase();
    for (var name in dict) {
      if (Object.prototype.hasOwnProperty.call(dict, name) && String(dict[name]).toLowerCase() === target) return name;
    }
    return undefined;
  },
  addValueToDict: function (dict, key, value) {
    if (dict[key] === undefined) dict[key] = value;
    return dict[key];
  },
  setStorage: async function (keyOrObj, value) {
    if (keyOrObj && typeof keyOrObj === "object") {
      for (var key in keyOrObj) {
        if (Object.prototype.hasOwnProperty.call(keyOrObj, key)) __uz_storage[key] = keyOrObj[key];
      }
    } else {
      __uz_storage[String(keyOrObj)] = value;
    }
  },
  getStorage: async function (keyOrObj) {
    if (keyOrObj && typeof keyOrObj === "object") {
      var out = {};
      for (var key in keyOrObj) {
        if (Object.prototype.hasOwnProperty.call(keyOrObj, key)) {
          out[key] = __uz_storage[key] !== undefined ? __uz_storage[key] : keyOrObj[key];
        }
      }
      return out;
    }
    var value = __uz_storage[String(keyOrObj)];
    return value === undefined ? "" : value;
  },
};

async function getEnv(uzTag, key) {
  var value = __uz_env[String(uzTag) + String(key)];
  return value === undefined ? "" : value;
}
async function setEnv(uzTag, key, value) {
  __uz_env[String(uzTag) + String(key)] = value;
}
async function goToVerify() {
  throw new Error("视频源兼容层暂不支持网页验证（goToVerify）");
}
async function openWebToBindEnv() {
  throw new Error("视频源兼容层暂不支持网页绑定（openWebToBindEnv）");
}
function toast() {}
var ToastType = { normal: "normal", info: "info", success: "success", error: "error" };
var kIsDesktop = true;
var kIsAndroid = false;
var kIsIOS = false;
var kIsWindows = true;
var kIsMacOS = false;
var kIsTV = false;
var kLocale = "zh-CN";
var kAppVersion = 1643;
function formatBackData(params) { return JSON.stringify(params); }

var JSONbig = { parse: function (value) { return JSON.parse(value); }, stringify: function (value) { return JSON.stringify(value); } };

// ── CryptoJS 兼容层（2026-10-29）────────────────────────────────────
// uzVideo 的 Crypto 就是 CryptoJS（core/core/uz3lib.js 里 createCryptoJS()），
// 视频扩展按 Crypto.MD5(...).toString() / Crypto.AES.encrypt(...) 使用。
// 这里只实现扩展实际用到的子集：WordArray、Utf8/Hex/Base64 编码器、MD5 / SHA1 /
// SHA256 / SHA512、HmacSHA256、AES（CBC / ECB + Pkcs7 / NoPadding）。
// Encrypt 是 JSEncrypt（RSA），只有网盘工具（type 300）用，不在支持范围内。
function __waBytes(value) {
  if (value && value.__wordArray) {
    var out = new Uint8Array(value.sigBytes);
    for (var i = 0; i < value.sigBytes; i++) {
      out[i] = (value.words[i >>> 2] >>> (24 - (i % 4) * 8)) & 0xff;
    }
    return out;
  }
  return __toBytes(value);
}
function __wa(value) {
  var bytes = __waBytes(value);
  var words = [];
  for (var i = 0; i < bytes.length; i++) words[i >>> 2] |= bytes[i] << (24 - (i % 4) * 8);
  return {
    __wordArray: true,
    words: words,
    sigBytes: bytes.length,
    toString: function (encoder) { return (encoder || __encHex).stringify(this); },
    concat: function (other) {
      var left = __waBytes(this);
      var right = __waBytes(other);
      var merged = new Uint8Array(left.length + right.length);
      merged.set(left);
      merged.set(right, left.length);
      var next = __wa(merged);
      this.words = next.words;
      this.sigBytes = next.sigBytes;
      return this;
    },
    clamp: function () {
      var next = __wa(__waBytes(this));
      this.words = next.words;
      this.sigBytes = next.sigBytes;
    },
  };
}
var __encHex = {
  stringify: function (wa) { return __hex(__waBytes(wa)); },
  parse: function (text) {
    var clean = String(text).replace(/[^0-9a-fA-F]/g, "");
    var out = new Uint8Array(Math.floor(clean.length / 2));
    for (var i = 0; i < out.length; i++) out[i] = parseInt(clean.substr(i * 2, 2), 16);
    return __wa(out);
  },
};
var __encBase64 = {
  stringify: function (wa) { return __lunac_b64encode(__bytesToBuffer(__waBytes(wa))); },
  parse: function (text) { return __wa(new Uint8Array(__lunac_b64decode(text))); },
};
var __encUtf8 = {
  stringify: function (wa) { return __lunac_utf8_decode(__bytesToBuffer(__waBytes(wa))); },
  parse: function (text) { return __wa(new Uint8Array(__lunac_utf8(text))); },
};
function __hashWa(name, input) {
  return __wa(new Uint8Array(__hashBytes(name, __waBytes(input))));
}
function __pkcs7Pad(bytes) {
  var pad = 16 - (bytes.length % 16);
  var out = new Uint8Array(bytes.length + pad);
  out.set(bytes);
  for (var i = bytes.length; i < out.length; i++) out[i] = pad;
  return out;
}
function __pkcs7Unpad(bytes) {
  if (!bytes.length) return bytes;
  var pad = bytes[bytes.length - 1];
  if (pad < 1 || pad > 16 || pad > bytes.length) return bytes;
  return bytes.subarray(0, bytes.length - pad);
}
function __cipherBytes(cipher) {
  if (typeof cipher === "string") return new Uint8Array(__lunac_b64decode(cipher));
  if (cipher && cipher.ciphertext) return __waBytes(cipher.ciphertext);
  return __waBytes(cipher);
}
function __aesRun(encrypting, data, key, cfg) {
  var mode = (cfg && cfg.mode) || Crypto.mode.CBC;
  var padding = (cfg && cfg.padding) || Crypto.pad.Pkcs7;
  var keyBytes = __waBytes(key);
  var ecb = mode === Crypto.mode.ECB;
  if (encrypting) {
    var bytes = data;
    if (padding === Crypto.pad.Pkcs7) bytes = __pkcs7Pad(bytes);
    if (ecb) return new Uint8Array(__aesEcbEncrypt(bytes, keyBytes));
    return new Uint8Array(__aesCbcEncrypt(bytes, keyBytes, __waBytes(cfg && cfg.iv)));
  }
  var plain = ecb
    ? new Uint8Array(__aesEcbDecrypt(data, keyBytes))
    : new Uint8Array(__aesCbcDecrypt(data, keyBytes, __waBytes(cfg && cfg.iv)));
  return padding === Crypto.pad.Pkcs7 ? __pkcs7Unpad(plain) : plain;
}
var Crypto = {
  lib: {
    WordArray: {
      create: function (input) { return __wa(input || new Uint8Array(0)); },
      random: function (nBytes) {
        var out = new Uint8Array(nBytes);
        for (var i = 0; i < nBytes; i++) out[i] = Math.floor(Math.random() * 256);
        return __wa(out);
      },
    },
  },
  enc: { Utf8: __encUtf8, Hex: __encHex, Base64: __encBase64 },
  mode: { CBC: { name: "CBC" }, ECB: { name: "ECB" } },
  pad: { Pkcs7: { name: "Pkcs7" }, NoPadding: { name: "NoPadding" } },
  MD5: function (input) { return __hashWa("md5", input); },
  SHA1: function (input) { return __hashWa("sha1", input); },
  SHA256: function (input) { return __hashWa("sha256", input); },
  SHA512: function (input) { return __hashWa("sha512", input); },
  HmacSHA256: function (message, key) {
    return __wa(new Uint8Array(__hmacBytes("sha256", __waBytes(key), __waBytes(message))));
  },
  AES: {
    encrypt: function (message, key, cfg) {
      var bytes = __aesRun(true, __waBytes(message), key, cfg);
      var wa = __wa(bytes);
      return {
        ciphertext: wa,
        toString: function (encoder) { return (encoder || __encBase64).stringify(wa); },
      };
    },
    decrypt: function (cipher, key, cfg) {
      return __wa(__aesRun(false, __cipherBytes(cipher), key, cfg));
    },
  },
};

// cheerio 的 $(sel)[0] 给的是裸节点，上游脚本会读 .attribs / .children[0].data / .name。
// 内部节点用 __view 包一层只读视图（子节点懒展开），$() 再把 .node 包回集合。
function __view(node) {
  if (node.__view) return node.__view;
  var attrs = node.attrs || {};
  var view = {
    node: node,
    type: node.type,
    name: node.tag,
    tag: node.tag,
    attribs: attrs,
    attrs: attrs,
  };
  Object.defineProperty(view, "data", {
    get: function () {
      return node.type === "text" ? __decode(node.text) : __textOf(node);
    },
  });
  Object.defineProperty(view, "children", {
    get: function () {
      var out = [];
      for (var i = 0; i < node.children.length; i++) out.push(__view(node.children[i]));
      return out;
    },
  });
  node.__view = view;
  return view;
}

function __CheerioCollection(elements, doc) {
  this.elements = elements || [];
  this.doc = doc;
  this.length = this.elements.length;
  // 下标访问（$(…)[0]）在真实扩展里非常常见；上限 64 避免大列表逐个建视图。
  for (var i = 0; i < this.length && i < 64; i++) {
    var element = this.elements[i];
    this[i] = __view(element && element.node ? element.node : element);
  }
}
__CheerioCollection.prototype.each = function (callback) {
  for (var i = 0; i < this.elements.length; i++) {
    if (callback.call(this.elements[i], i, this.elements[i]) === false) break;
  }
  return this;
};
__CheerioCollection.prototype.map = function (callback) {
  var values = [];
  for (var i = 0; i < this.elements.length; i++) values.push(callback.call(this.elements[i], i, this.elements[i]));
  return {
    values: values,
    length: values.length,
    get: function (index) { return index === undefined ? values : values[index]; },
    toArray: function () { return values; },
    join: function (separator) { return values.join(separator); },
  };
};
__CheerioCollection.prototype.get = function (index) { return this.elements[index]; };
__CheerioCollection.prototype.toArray = function () { return this.elements.slice(); };
__CheerioCollection.prototype.eq = function (index) {
  return new __CheerioCollection(this.elements[index] ? [this.elements[index]] : [], this.doc);
};
__CheerioCollection.prototype.first = function () { return this.eq(0); };
__CheerioCollection.prototype.last = function () { return this.eq(this.elements.length - 1); };
__CheerioCollection.prototype.find = function (selector) {
  var out = [];
  for (var i = 0; i < this.elements.length; i++) {
    var found = this.elements[i].querySelectorAll(selector);
    for (var j = 0; j < found.length; j++) out.push(found[j]);
  }
  return new __CheerioCollection(out, this.doc);
};
__CheerioCollection.prototype.children = function (selector) {
  var out = [];
  var matches = selector ? this.doc.querySelectorAll(selector) : null;
  for (var i = 0; i < this.elements.length; i++) {
    var children = this.elements[i].children;
    for (var j = 0; j < children.length; j++) {
      if (!matches || matches.indexOf(children[j]) >= 0) out.push(children[j]);
    }
  }
  return new __CheerioCollection(out, this.doc);
};
__CheerioCollection.prototype.parent = function () {
  var out = [];
  for (var i = 0; i < this.elements.length; i++) {
    if (this.elements[i].parent) out.push(this.elements[i].parent);
  }
  return new __CheerioCollection(out, this.doc);
};
__CheerioCollection.prototype.next = function () {
  var out = [];
  for (var i = 0; i < this.elements.length; i++) {
    if (this.elements[i].nextElementSibling) out.push(this.elements[i].nextElementSibling);
  }
  return new __CheerioCollection(out, this.doc);
};
__CheerioCollection.prototype.prev = function () {
  var out = [];
  for (var i = 0; i < this.elements.length; i++) {
    if (this.elements[i].previousElementSibling) out.push(this.elements[i].previousElementSibling);
  }
  return new __CheerioCollection(out, this.doc);
};
__CheerioCollection.prototype.text = function () {
  var out = "";
  for (var i = 0; i < this.elements.length; i++) out += this.elements[i].text;
  return out;
};
__CheerioCollection.prototype.html = function () {
  var out = "";
  for (var i = 0; i < this.elements.length; i++) out += this.elements[i].innerHTML;
  return out;
};
__CheerioCollection.prototype.attr = function (name) {
  if (!this.elements.length) return undefined;
  return this.elements[0].attributes[String(name).toLowerCase()];
};
__CheerioCollection.prototype.hasClass = function (name) {
  for (var i = 0; i < this.elements.length; i++) {
    if (this.elements[i].classNames.indexOf(String(name)) >= 0) return true;
  }
  return false;
};

function __cheerioLoad(html) {
  var doc = new HtmlDocument(html);
  function $(selector) {
    if (typeof selector === "string") return new __CheerioCollection(doc.querySelectorAll(selector), doc);
    if (selector && selector.node) return new __CheerioCollection([__wrap(selector.node)], doc);
    if (selector && selector.elements) return selector;
    return new __CheerioCollection([], doc);
  }
  $.root = function () { return new __CheerioCollection(doc.querySelectorAll("html"), doc); };
  $.html = function () { return __serialize(doc.root); };
  return $;
}
var cheerio = { load: __cheerioLoad };
`;

// ── 视频源 调用入口（拼在上游脚本之后）────────────────────────
export const VIDEO_RUNNER = String.raw`
function __uz_parse(raw) {
  if (raw === undefined || raw === null) return {};
  if (typeof raw !== "string") return raw;
  try {
    return JSON.parse(raw);
  } catch (error) {
    throw new Error("视频源 返回了非 JSON：" + raw.slice(0, 120));
  }
}
function __uz_payload(raw) {
  var parsed = __uz_parse(raw);
  if (parsed && parsed.error) throw new Error(String(parsed.error));
  return parsed || {};
}
function __uz_plain(value) {
  return String(value == null ? "" : value)
    .replace(/<[^>]*>/g, " ")
    .replace(/&nbsp;/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}
function __uz_tags(item) {
  var out = [];
  var raw = [item.vod_class, item.type_name, item.vod_sub];
  for (var i = 0; i < raw.length; i++) {
    if (!raw[i]) continue;
    var parts = String(raw[i]).split(/[,，/|]/);
    for (var j = 0; j < parts.length; j++) {
      var tag = parts[j].trim();
      if (tag && out.indexOf(tag) < 0) out.push(tag);
    }
  }
  return out.length ? out : undefined;
}
function __uz_work(item) {
  return {
    id: String(item.vod_id != null ? item.vod_id : (item.id != null ? item.id : "")),
    title: String(item.vod_name != null ? item.vod_name : (item.title || "")),
    author: item.vod_director ? String(item.vod_director) : undefined,
    cover: item.vod_pic ? String(item.vod_pic) : undefined,
    description: __uz_plain(item.vod_blurb || item.vod_content || item.vod_remarks || item.description) || undefined,
    tags: __uz_tags(item),
  };
}
// 上游首页 / 分类接口常见几种形态：数组、{data:[...]}、{list:[...]}、{videos:[...]}。
function __uz_list(raw) {
  if (Array.isArray(raw)) return raw;
  var parsed = __uz_payload(raw);
  if (Array.isArray(parsed)) return parsed;
  if (!parsed || typeof parsed !== "object") return [];
  if (Array.isArray(parsed.data)) return parsed.data;
  if (Array.isArray(parsed.list)) return parsed.list;
  if (Array.isArray(parsed.videos)) return parsed.videos;
  if (Array.isArray(parsed.class)) return parsed.class;
  return [];
}
function __uz_homeItems(raw) {
  var list = __uz_list(raw);
  var out = [];
  for (var i = 0; i < list.length; i++) {
    var item = list[i] || {};
    // 分区形态：{ type_name, videos / data / list }
    var nested = item.videos || item.data || item.list;
    if (Array.isArray(nested)) {
      for (var j = 0; j < nested.length; j++) out.push(nested[j]);
    } else {
      out.push(item);
    }
  }
  return out;
}

async function __uz_main() {
  if (args.method === "search") {
    if (typeof searchVideo !== "function") throw new Error("该视频源没有实现 searchVideo");
    var list = __uz_payload(await searchVideo({ url: "", page: args.page || 1, searchWord: args.query || "", flag: "" }));
    var items = (list && list.data) || [];
    return { items: items.map(__uz_work) };
  }

  if (args.method === "explore") {
    var section = args.section ? String(args.section) : "";
    var seen = {};
    var items = [];
    function collect(list) {
      for (var i = 0; i < list.length; i++) {
        var work = __uz_work(list[i] || {});
        if (!work.id && !work.title) continue;
        var key = work.id || work.title;
        if (seen[key]) continue;
        seen[key] = true;
        items.push(work);
      }
    }
    async function collectClasses() {
      var classes = __uz_list(await getClassList());
      // 总览一次最多聚合前几个分区，避免单个来源把首屏拖慢。
      var limit = Math.min(classes.length, 4);
      for (var c = 0; c < limit; c++) {
        var cls = classes[c] || {};
        var typeId = cls.type_id != null ? cls.type_id : (cls.id != null ? cls.id : "");
        if (typeId === "") continue;
        // 上游 UZArgs 的分类标识放在 args.url；type_id 仅为旧脚本兼容。
        collect(__uz_list(await getVideoList({ url: String(typeId), type_id: String(typeId), page: 1, filter: {} })));
      }
    }
    if (section) {
      if (typeof getVideoList !== "function") throw new Error("该视频源没有实现 getVideoList");
      collect(__uz_list(await getVideoList({ url: String(section), type_id: section, page: args.page || 1, filter: {} })));
    } else if (typeof getHome === "function") {
      collect(__uz_homeItems(await getHome()));
      // 首页只有轮播 / 空数据时，退回分类列表，避免总览该源整块空白。
      if (!items.length && typeof getClassList === "function" && typeof getVideoList === "function") {
        await collectClasses();
      }
    } else if (typeof getClassList === "function" && typeof getVideoList === "function") {
      await collectClasses();
    } else {
      throw new Error("该视频源没有实现 getHome / getClassList");
    }
    return { items: items };
  }

  if (args.method === "detail") {
    if (typeof getVideoDetail !== "function") throw new Error("该视频源没有实现 getVideoDetail");
    var info = __uz_payload(await getVideoDetail({ url: args.workId, page: 1, searchWord: "", flag: "" }));
    var infoData = Array.isArray(info.data) ? info.data[0] : info.data;
    return { work: __uz_work(infoData || {}) };
  }

  if (args.method === "chapters") {
    if (typeof getVideoDetail !== "function") throw new Error("该视频源没有实现 getVideoDetail");
    var detail = __uz_payload(await getVideoDetail({ url: args.workId, page: 1, searchWord: "", flag: "" }));
    var data = detail.data || {};
    var groups = String(data.vod_play_from || "").split("$$$");
    var urls = String(data.vod_play_url || "").split("$$$");
    var chapters = [];
    for (var g = 0; g < urls.length; g++) {
      var episodes = String(urls[g]).split("#");
      for (var e = 0; e < episodes.length; e++) {
        var episode = episodes[e];
        if (!episode) continue;
        var split = episode.indexOf("$");
        var title = split >= 0 ? episode.slice(0, split) : episode;
        var link = split >= 0 ? episode.slice(split + 1) : episode;
        if (!link) continue;
        chapters.push({
          id: (groups[g] || "") + "\u0001" + link,
          title: (groups.length > 1 && groups[g] ? groups[g] + " · " : "") + (title || ("第 " + (e + 1) + " 集")),
          group: groups[g] || undefined,
        });
      }
    }
    return { chapters: chapters };
  }

  if (args.method === "play") {
    if (typeof getVideoPlayUrl !== "function") throw new Error("该视频源没有实现 getVideoPlayUrl");
    var splitIndex = String(args.chapterId).indexOf("\u0001");
    var flag = splitIndex >= 0 ? String(args.chapterId).slice(0, splitIndex) : "";
    var episodeUrl = splitIndex >= 0 ? String(args.chapterId).slice(splitIndex + 1) : String(args.chapterId);
    var play = __uz_payload(await getVideoPlayUrl({ url: episodeUrl, flag: flag, page: 1, searchWord: "" }));
    var candidates = Array.isArray(play.urls) ? play.urls : [];
    var playbacks = [];
    for (var i = 0; i < candidates.length; i++) {
      var candidate = candidates[i];
      if (!candidate) continue;
      var candidateUrl = typeof candidate === "string" ? candidate : (candidate.url || candidate.data);
      if (!candidateUrl) continue;
      playbacks.push({
        url: String(candidateUrl),
        title: typeof candidate === "object" ? candidate.name : undefined,
        headers: (typeof candidate === "object" && candidate.headers) || play.headers || {},
        subtitleUrl: typeof candidate === "object" ? candidate.subtitleUrl : undefined,
      });
    }
    if (!playbacks.length && play.data) {
      playbacks.push({ url: String(play.data), headers: play.headers || {} });
    }
    if (!playbacks.length) throw new Error("视频源 没有返回可播放地址");
    return { playbacks: playbacks };
  }

  throw new Error("视频源兼容层不支持方法：" + args.method);
}

__uz_main()
`;

