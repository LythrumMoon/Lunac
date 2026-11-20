// 平台薄层单测：版本比较、清单校验与 SHA-256 标准向量（不联网）。
import 'package:flutter_test/flutter_test.dart';
import 'package:lunac/platform/sha256.dart';
import 'package:lunac/platform/updater.dart';

void main() {
  test('sha256 matches the standard test vectors', () {
    // FIPS 180-4 附录 B / 常用向量：空串
    expect(
      sha256Hex(const <int>[]),
      'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
    );
    // 'abc'
    expect(
      sha256Hex('abc'.codeUnits),
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
    );
    // 100 万个 'a'（跨多个块，顺带覆盖长度补位）
    final millionA = List<int>.filled(1000 * 1000, 0x61);
    expect(
      sha256Hex(millionA),
      'cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0',
    );
  });

  test('sha256 incremental update matches one-shot digest', () {
    final hasher = Sha256()
      ..update('ab'.codeUnits)
      ..update('c'.codeUnits);
    expect(
      hasher.hexDigest(),
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
    );
  });

  test('version parsing accepts repo style and rejects junk', () {
    expect(parseVersion('0.9.45'), [0, 9, 45]);
    expect(parseVersion('v1.2.3'), [1, 2, 3]);
    expect(parseVersion(' 0.9.7 '), [0, 9, 7]);
    // 只写两段：第三段补 0
    expect(parseVersion('1.4'), [1, 4, 0]);
    // 预发布 / 非数字 / 段数不对：解析不出来
    expect(parseVersion('0.9.7-rc1'), isNull);
    expect(parseVersion(''), isNull);
    expect(parseVersion('abc'), isNull);
    expect(parseVersion('1.2.3.4'), isNull);
  });

  test('newer requires strictly greater and never guesses', () {
    expect(isNewerVersion('0.9.45', '0.9.46'), isTrue);
    expect(isNewerVersion('0.9.45', '0.10.0'), isTrue);
    expect(isNewerVersion('0.9.45', '1.0.0'), isTrue);
    // 同版本 / 更旧：都不是更新（同版本那条尤其重要，否则会重复提示）
    expect(isNewerVersion('0.9.45', '0.9.45'), isFalse);
    expect(isNewerVersion('0.9.46', '0.9.45'), isFalse);
    // 任一侧解析不出来 ⇒ false
    expect(isNewerVersion('0.9.45', '0.9.46-rc1'), isFalse);
    expect(isNewerVersion('dev', '0.9.46'), isFalse);
  });

  const good = UpdateManifest(
    version: '0.9.46',
    notes: 'fix',
    url: 'https://github.com/LythrumMoon/Lunac/releases/download/v0.9.46/'
        'Lunac-0.9.46-Setup.exe',
    sha256: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    size: 123,
  );

  test('parses the real published manifest shape', () {
    // 与 2026-11-10 线上 latest.json 完全同形（0.9.45 是最新 Release）。
    const text = '{"version":"0.9.45","notes":"Lunac v0.9.45",'
        '"url":"https://github.com/LythrumMoon/Lunac/releases/download/v0.9.45/'
        'Lunac-0.9.45-Setup.exe",'
        '"sha256":"6897b3e8d3d94a9a0aa83849964b86f04e7021fda4f9faa97f93849f47acd0ea",'
        '"size":22635957}';
    final result = parseUpdateManifest(text);
    expect(result.error, isNull);
    expect(result.status!.latest, '0.9.45');
    expect(result.status!.current, kAppVersion);
    expect(result.status!.size, 22635957);
    expect(
      result.status!.sha256,
      '6897b3e8d3d94a9a0aa83849964b86f04e7021fda4f9faa97f93849f47acd0ea',
    );
    expect(result.status!.hasUpdate, isFalse);
  });

  test('manifest parsing tolerates a BOM and rejects junk', () {
    // BOM 不算格式错误（错误应来自字段校验，而不是 JSON 解析）。
    final bom = parseUpdateManifest('\uFEFF{"version":"0.9.45"}');
    expect(bom.error, isNotNull);
    expect(bom.error, isNot(contains('格式不对')));

    expect(parseUpdateManifest('not json').error, contains('格式不对'));
    expect(parseUpdateManifest('[]').error, contains('格式不对'));
  });

  test('manifest validation rejects unsafe sources', () {
    expect(validateManifest(good), isNull);

    // 明文 http
    expect(
      validateManifest(
        UpdateManifest(
          version: good.version,
          notes: '',
          url: good.url.replaceFirst('https://', 'http://'),
          sha256: good.sha256,
          size: good.size,
        ),
      ),
      isNotNull,
    );

    // 不是 GitHub 的域
    expect(
      validateManifest(
        const UpdateManifest(
          version: '0.9.46',
          notes: '',
          url: 'https://evil.example.com/Lunac-Setup.exe',
          sha256: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
          size: 0,
        ),
      ),
      isNotNull,
    );

    // sha256 缺失 / 长度不对 / 非十六进制
    for (final bad in ['', 'abc', 'z' * 64]) {
      expect(
        validateManifest(
          UpdateManifest(
            version: good.version,
            notes: '',
            url: good.url,
            sha256: bad,
            size: 0,
          ),
        ),
        isNotNull,
        reason: 'sha256=$bad 应被拒',
      );
    }

    // 版本号解析不出来
    expect(
      validateManifest(
        UpdateManifest(
          version: 'latest',
          notes: '',
          url: good.url,
          sha256: good.sha256,
          size: 0,
        ),
      ),
      isNotNull,
    );
  });

  test('auto update defaults to on and needs both fields', () {
    // v1 2026-10-05 合并语义：两个字段同真才算开，默认开。
    expect(UpdateConfig.defaults.enabled, isTrue);
    const onlyCheck = UpdateConfig(checkOnStartup: true, autoInstall: false);
    const onlyInstall = UpdateConfig(checkOnStartup: false, autoInstall: true);
    const off = UpdateConfig(checkOnStartup: false, autoInstall: false);
    expect(onlyCheck.enabled, isFalse);
    expect(onlyInstall.enabled, isFalse);
    expect(off.enabled, isFalse);
  });
}
