using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Threading.Tasks;
using DSPRemoteSaves;

namespace CSharpInterop
{
    /// <summary>
    /// 端到端验证：按 DSPRemoteSaves.cs 插件的真实协议对服务器执行
    /// 公钥获取 → 国密封装 → init → 分片(uploadToken) → complete → file-list → download 回读。
    /// 用法：cstest e2e &lt;baseUrl&gt; &lt;user&gt; &lt;password&gt;
    /// </summary>
    public static class E2E
    {
        private static readonly JsonSerializerOptions JsonOpts = new JsonSerializerOptions
        {
            PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
            PropertyNameCaseInsensitive = true,
        };

        private sealed class ApiBody
        {
            public int Code { get; set; }
            public string Message { get; set; }
            public JsonElement Payload { get; set; }
        }

        public static async Task<int> Run(string baseUrl, string user, string password)
        {
            if (!baseUrl.EndsWith("/")) baseUrl += "/";
            using var http = new HttpClient();
            var rand = RandomNumberGenerator.Create();

            Console.WriteLine($"[E2E] 目标 {baseUrl} 用户 {user}");

            // 1) 公钥
            var pkResp = await http.GetAsync(baseUrl + "crypto/public-key");
            var pk = JsonSerializer.Deserialize<ApiBody>(await pkResp.Content.ReadAsStringAsync(), JsonOpts);
            if (pk.Code != 0) throw new Exception("获取公钥失败: " + pk.Message);
            var keyId = pk.Payload.GetProperty("keyId").GetString();
            var publicKey = pk.Payload.GetProperty("publicKey").GetString();
            Console.WriteLine($"[1] 公钥 keyId={keyId}");

            // 模拟存档内容（伪随机、可复现）
            const int size = 384 * 1024;
            var content = new byte[size];
            uint seed = 0x1234567u;
            for (int i = 0; i < size; i++)
            {
                seed = seed * 1664525u + 1013904223u;
                content[i] = (byte)(seed >> 24);
            }
            var fileName = "csharp-e2e-存档.dsv";
            Console.WriteLine($"[2] 内容 {fileName} {size} 字节 sha256={Sha(content[..Math.Min(64, size)])[..12]}…");

            // 2) init（国密封装密码 + smkey/smkeyid 头，与插件 SealAsync 一致）
            var body = await PostSealed(http, baseUrl, "upload/init", publicKey, keyId, user, password, new Dictionary<string, object>
            {
                ["fileName"] = fileName,
                ["fileSize"] = size,
                ["chunkSize"] = 128 * 1024,
            });
            var data = body.Payload.GetProperty("data");
            var fileId = data.GetProperty("fileId").GetString();
            var chunkSize = data.GetProperty("chunkSize").GetInt32();
            var totalChunks = data.GetProperty("totalChunks").GetInt32();
            var uploadToken = data.GetProperty("uploadToken").GetString();
            Console.WriteLine($"[3] init fileId={fileId} chunkSize={chunkSize} totalChunks={totalChunks}");

            // 3) 分片：只带 uploadToken（与插件 UploadChunks 一致）
            for (int i = 0; i < totalChunks; i++)
            {
                var offset = (long)i * chunkSize;
                var len = (int)Math.Min(chunkSize, size - offset);
                using var chunkContent = new ByteArrayContent(content, (int)offset, len);
                chunkContent.Headers.ContentType = new System.Net.Http.Headers.MediaTypeHeaderValue("application/octet-stream");
                using var req = new HttpRequestMessage(HttpMethod.Post, baseUrl + $"upload/chunk?fileId={fileId}&chunkIndex={i}") { Content = chunkContent };
                req.Headers.TryAddWithoutValidation("uploadToken", uploadToken);
                var resp = await http.SendAsync(req);
                var text = await resp.Content.ReadAsStringAsync();
                var ok = JsonSerializer.Deserialize<ApiBody>(text, JsonOpts);
                if (!resp.IsSuccessStatusCode || ok.Code != 0) throw new Exception($"chunk {i} 失败: HTTP {(int)resp.StatusCode} {text}");
                Console.WriteLine($"[4] chunk {i + 1}/{totalChunks} 上传成功 ({len} 字节)");
            }

            // 4) complete
            body = await PostSealed(http, baseUrl, "upload/complete", publicKey, keyId, user, password, new Dictionary<string, object> { ["fileId"] = fileId });
            Console.WriteLine($"[5] complete code={body.Code}");

            // 5) file-list
            body = await PostSealed(http, baseUrl, "download/file-list", publicKey, keyId, user, password, new Dictionary<string, object>
            {
                ["syncFileExtensions"] = new List<string>(),
            });
            var files = body.Payload.GetProperty("files").EnumerateArray().ToList();
            var mine = files.FirstOrDefault(f => f.GetProperty("fileName").GetString() == fileName);
            if (mine.ValueKind == JsonValueKind.Undefined) throw new Exception("file-list 中未找到上传文件");
            Console.WriteLine($"[6] file-list 命中: {mine.GetProperty("fileName").GetString()} {mine.GetProperty("fileSize").GetInt64()} 字节");

            // 6) download 回读（流式 POST，与插件 PostSealedStreamAsync 一致）
            var (smkeyD, cipherD) = Seal(publicKey, password);
            var dlJson = JsonSerializer.Serialize(new Dictionary<string, object>
            {
                ["userName"] = user,
                ["password"] = cipherD,
                ["fileName"] = fileName,
                ["isChunk"] = false,
            });
            using var dlReq = new HttpRequestMessage(HttpMethod.Post, baseUrl + "download/download")
            {
                Content = new StringContent(dlJson, Encoding.UTF8, "application/json"),
            };
            dlReq.Headers.TryAddWithoutValidation("smkey", smkeyD);
            dlReq.Headers.TryAddWithoutValidation("smkeyid", keyId);
            using var dlResp = await http.SendAsync(dlReq, HttpCompletionOption.ResponseHeadersRead);
            if (!dlResp.IsSuccessStatusCode) throw new Exception("下载失败 HTTP " + (int)dlResp.StatusCode);
            var back = await dlResp.Content.ReadAsByteArrayAsync();
            var same = back.Length == size && back.AsSpan().SequenceEqual(content);
            Console.WriteLine($"[7] 下载 {back.Length} 字节，逐字节比对 {(same ? "一致 ✓" : "不一致 ✗")}");
            if (!same) throw new Exception("下载内容与上传不一致");

            Console.WriteLine("E2E RESULT: PASS");
            return 0;
        }

        private static (string smkey, string cipher) Seal(string publicKey, string plain)
        {
            SmCrypto.SealPassword(publicKey, plain, out var smkey, out var cipher);
            return (smkey, cipher);
        }

        private static async Task<ApiBody> PostSealed(
            HttpClient http, string baseUrl, string path,
            string publicKey, string keyId, string user, string password,
            Dictionary<string, object> extra)
        {
            var (smkey, cipher) = Seal(publicKey, password);
            var payload = new Dictionary<string, object> { ["userName"] = user, ["password"] = cipher };
            foreach (var kv in extra) payload[kv.Key] = kv.Value;
            using var req = new HttpRequestMessage(HttpMethod.Post, baseUrl + path)
            {
                Content = new StringContent(JsonSerializer.Serialize(payload), Encoding.UTF8, "application/json"),
            };
            req.Headers.TryAddWithoutValidation("smkey", smkey);
            req.Headers.TryAddWithoutValidation("smkeyid", keyId);
            var resp = await http.SendAsync(req);
            var text = await resp.Content.ReadAsStringAsync();
            var body = JsonSerializer.Deserialize<ApiBody>(text, JsonOpts);
            if (!resp.IsSuccessStatusCode || body.Code != 0)
                throw new Exception($"{path} 失败: HTTP {(int)resp.StatusCode} {text}");
            return body;
        }

        private static string Sha(byte[] data)
        {
            using var sha = SHA256.Create();
            return Convert.ToHexString(sha.ComputeHash(data)).ToLowerInvariant();
        }
    }
}
