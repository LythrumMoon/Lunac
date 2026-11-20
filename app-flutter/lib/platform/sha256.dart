// 平台薄层：纯 Dart SHA-256（FIPS 180-4）。
//
// 为什么不引入 `crypto` 包：迁移期的薄层刻意不新增 pub 依赖 —— 自更新只在 Windows
// 安装版上跑，多一个依赖就多一处构建 / 审计面。实现按 FIPS 180-4 §6.2 写：
// 64 字节一块、16 个 w 扩展到 64 个，只做「够校验安装包」的标准实现，不做额外优化。
//
// 用法：`sha256Hex(bytes)` 一次性算；边下边算用 `Sha256()..update(chunk)` + `hexDigest()`。

/// SHA-256 常量表（前 64 个质数立方根小数部分的前 32 位，FIPS 180-4 §4.2.2）。
const List<int> _k = [
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, //
  0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
  0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
  0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
  0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
  0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
  0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
  0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
  0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// 初始哈希值（前 8 个质数平方根小数部分的前 32 位，FIPS 180-4 §5.3.3）。
const List<int> _initialHash = [
  0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, //
  0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// 32 位循环右移（Dart 的 int 是 64 位，移位后要截回 32 位）。
int _rotr(int value, int count) {
  return ((value >> count) | (value << (32 - count))) & 0xFFFFFFFF;
}

/// 增量式 SHA-256：`update` 可以按任意块大小调用，`hexDigest` 输出小写十六进制。
class Sha256 {
  final List<int> _hash = List<int>.of(_initialHash);

  /// 还没凑满 64 字节的尾巴。
  final List<int> _buffer = <int>[];

  /// 已喂入的字节数（哈希前的消息长度按位填在末尾）。
  int _bytesHashed = 0;

  /// 结果缓存：`hexDigest` 会补位、改内部状态，只允许取一次。
  String? _digest;

  /// 喂入一段数据（值是 0..255 的字节）。
  void update(List<int> data) {
    if (_digest != null) {
      throw StateError('hexDigest() 之后不能再 update');
    }
    _bytesHashed += data.length;
    var offset = 0;
    if (_buffer.isNotEmpty) {
      final need = 64 - _buffer.length;
      final take = data.length < need ? data.length : need;
      _buffer.addAll(data.sublist(0, take));
      offset = take;
      if (_buffer.length == 64) {
        _processBlock(_buffer, 0);
        _buffer.clear();
      }
    }
    while (data.length - offset >= 64) {
      _processBlock(data, offset);
      offset += 64;
    }
    if (offset < data.length) {
      _buffer.addAll(data.sublist(offset));
    }
  }

  /// 补齐消息并返回小写十六进制摘要（调用后本实例只可继续取同一个结果）。
  String hexDigest() {
    final cached = _digest;
    if (cached != null) {
      return cached;
    }
    final bitLength = _bytesHashed * 8;
    final padLength = _buffer.length < 56 ? 56 - _buffer.length : 120 - _buffer.length;
    final tail = List<int>.filled(padLength + 8, 0);
    tail[0] = 0x80;
    // 末尾 8 字节 = 消息位数的大端表示（512MB 安装包也不会溢出）。
    final high = bitLength ~/ 0x100000000;
    final low = bitLength & 0xFFFFFFFF;
    for (var i = 0; i < 4; i++) {
      tail[padLength + i] = (high >> (24 - 8 * i)) & 0xFF;
      tail[padLength + 4 + i] = (low >> (24 - 8 * i)) & 0xFF;
    }
    // 补位后总长一定是 64 的倍数，update 会把 _buffer 处理干净。
    update(tail);
    final text = _hash.map((word) => word.toRadixString(16).padLeft(8, '0')).join();
    _digest = text;
    return text;
  }

  void _processBlock(List<int> block, int offset) {
    final w = List<int>.filled(64, 0);
    for (var i = 0; i < 16; i++) {
      final at = offset + i * 4;
      w[i] = (block[at] << 24) |
          (block[at + 1] << 16) |
          (block[at + 2] << 8) |
          block[at + 3];
    }
    for (var i = 16; i < 64; i++) {
      final x = w[i - 15];
      final y = w[i - 2];
      final s0 = _rotr(x, 7) ^ _rotr(x, 18) ^ (x >> 3);
      final s1 = _rotr(y, 17) ^ _rotr(y, 19) ^ (y >> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) & 0xFFFFFFFF;
    }

    var a = _hash[0];
    var b = _hash[1];
    var c = _hash[2];
    var d = _hash[3];
    var e = _hash[4];
    var f = _hash[5];
    var g = _hash[6];
    var h = _hash[7];

    for (var i = 0; i < 64; i++) {
      final s1 = _rotr(e, 6) ^ _rotr(e, 11) ^ _rotr(e, 25);
      final ch = (e & f) ^ ((~e) & g);
      final temp1 = (h + s1 + ch + _k[i] + w[i]) & 0xFFFFFFFF;
      final s0 = _rotr(a, 2) ^ _rotr(a, 13) ^ _rotr(a, 22);
      final maj = (a & b) ^ (a & c) ^ (b & c);
      final temp2 = (s0 + maj) & 0xFFFFFFFF;
      h = g;
      g = f;
      f = e;
      e = (d + temp1) & 0xFFFFFFFF;
      d = c;
      c = b;
      b = a;
      a = (temp1 + temp2) & 0xFFFFFFFF;
    }

    _hash[0] = (_hash[0] + a) & 0xFFFFFFFF;
    _hash[1] = (_hash[1] + b) & 0xFFFFFFFF;
    _hash[2] = (_hash[2] + c) & 0xFFFFFFFF;
    _hash[3] = (_hash[3] + d) & 0xFFFFFFFF;
    _hash[4] = (_hash[4] + e) & 0xFFFFFFFF;
    _hash[5] = (_hash[5] + f) & 0xFFFFFFFF;
    _hash[6] = (_hash[6] + g) & 0xFFFFFFFF;
    _hash[7] = (_hash[7] + h) & 0xFFFFFFFF;
  }
}

/// 一次性算 SHA-256，返回小写十六进制。
String sha256Hex(List<int> bytes) {
  final hasher = Sha256()..update(bytes);
  return hasher.hexDigest();
}
