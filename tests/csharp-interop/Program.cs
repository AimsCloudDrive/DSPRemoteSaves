using System;
using System.Collections.Generic;
using System.IO;
using System.Text;
using System.Text.Json;
using DSPRemoteSaves;

internal static class Program
{
    private static int Main(string[] args)
    {
        // 自检模式：cstest selftest（与上传器的 net48 自检对照）
        if (args.Length >= 1 && args[0] == "selftest")
        {
            var sm3v = SmCrypto.BytesToHex(SmCrypto.Sm3(Encoding.UTF8.GetBytes("abc")));
            var sm4v = SmCrypto.Sm4EcbEncryptHex(
                "0123456789abcdeffedcba9876543210",
                SmCrypto.HexToBytes("0123456789abcdeffedcba9876543210"));
            Console.WriteLine("SM4[0..16]=" + sm4v.Substring(0, 32));
            return 0;
        }

        // e2e 模式：cstest e2e <baseUrl> <user> <password>
        if (args.Length >= 1 && args[0] == "e2e")
        {
            return CSharpInterop.E2E.Run(
                args.Length > 1 ? args[1] : "http://127.0.0.1:9999/dsp.saves/api/",
                args.Length > 2 ? args[2] : "admin",
                args.Length > 3 ? args[3] : "").GetAwaiter().GetResult();
        }

        return RunVectors(args);
    }

    private static int RunVectors(string[] args)
    {
        string fixture = args.Length > 0
            ? args[0]
            : Path.Combine("..", "..", "services", "acc-rs", "tests", "fixtures", "sm-vectors.json");
        string output = args.Length > 1
            ? args[1]
            : Path.Combine("..", "..", "services", "acc-rs", "tests", "fixtures", "csharp-vectors.json");

        // 1) SM3 标准向量
        string abc = SmCrypto.BytesToHex(SmCrypto.Sm3(Encoding.UTF8.GetBytes("abc")));
        const string expectedSm3 = "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0";
        Console.WriteLine($"SM3(abc) = {abc} {(abc == expectedSm3 ? "OK" : "MISMATCH")}");
        if (abc != expectedSm3) return 2;

        // 2) SM4 标准单块（PKCS#7 填充不影响首块）
        string sm4 = SmCrypto.Sm4EcbEncryptHex(
            "0123456789abcdeffedcba9876543210",
            SmCrypto.HexToBytes("0123456789abcdeffedcba9876543210"));
        Console.WriteLine($"SM4 first block = {sm4.Substring(0, 32)} {(sm4.StartsWith("681edf34d206965e86b3e94f536e4246") ? "OK" : "MISMATCH")}");
        if (!sm4.StartsWith("681edf34d206965e86b3e94f536e4246")) return 3;

        // 3) 用 JS 向量里的公钥加密 50 组明文，交给 Node 反向校验
        using var doc = JsonDocument.Parse(File.ReadAllText(fixture));
        string publicKey = doc.RootElement.GetProperty("publicKey").GetString();
        var cases = new List<object>();
        int index = 0;
        foreach (var item in doc.RootElement.GetProperty("cases").EnumerateArray())
        {
            if (index++ >= 50) break;
            string plain = item.GetProperty("plain").GetString();
            SmCrypto.SealPassword(publicKey, plain, out string smkey, out string cipher);
            cases.Add(new { plain, smkey, password = cipher });
        }

        var payload = new
        {
            generator = "SmCrypto.cs",
            publicKey,
            cases,
        };
        File.WriteAllText(output, JsonSerializer.Serialize(payload, new JsonSerializerOptions { WriteIndented = true }));
        Console.WriteLine($"wrote {cases.Count} vectors -> {Path.GetFullPath(output)}");
        return 0;
    }
}
