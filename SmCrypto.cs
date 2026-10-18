using System;
using System.Numerics;
using System.Security.Cryptography;
using System.Text;

namespace DSPRemoteSaves
{
    /// <summary>
    /// 自包含国密实现（SM2 加密 / SM3 / SM4-ECB-PKCS7），零外部依赖。
    ///
    /// 与服务端 `sm-envelope`、前端 `sm-crypto` 的约定完全一致：
    /// - SM2 曲线 sm2p256v1，密文 C1C3C2，C1 为未压缩点 04||X||Y
    /// - SM2 加密的明文是 SM4 密钥的 32 位 hex 字符串
    /// - SM4 使用 ECB + PKCS#7，密文小写 hex
    ///
    /// 说明：.NET Framework 4.8 没有内置国密，BouncyCastle 需要额外分发 DLL；
    /// 这里只实现“加密”方向，代码量小且便于随插件单文件分发。
    /// </summary>
    internal static class SmCrypto
    {
        // ---- sm2p256v1 曲线参数 ----
        private static readonly BigInteger P = FromHex(
            "FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFF");
        private static readonly BigInteger A = FromHex(
            "FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFC");
        private static readonly BigInteger B = FromHex(
            "28E9FA9E9D9F5E344D5A9E4BCF6509A7F39789F515AB8F92DDBCBD414D940E93");
        private static readonly BigInteger N = FromHex(
            "FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFF7203DF6B21C6052B53BBF40939D54123");
        private static readonly BigInteger Gx = FromHex(
            "32C4AE2C1F1981195F9904466A39C9948FE30BBFF2660BE1715A4589334C74C7");
        private static readonly BigInteger Gy = FromHex(
            "BC3736A2F4F6779C59BDCEE36B692153D0A9877CC62A474002DF32E52139F0A0");

        // ------------------------------------------------------------------
        // 对外 API
        // ------------------------------------------------------------------

        /// <summary>加密密码：返回 (smkey 头, SM4 密文)。</summary>
        public static void SealPassword(string publicKeyHex, string password, out string smkey, out string passwordCipher)
        {
            string sm4KeyHex = RandomHex(16);                     // 32 位 hex 字符串
            smkey = Sm2EncryptHex(publicKeyHex, Encoding.UTF8.GetBytes(sm4KeyHex));
            passwordCipher = Sm4EcbEncryptHex(sm4KeyHex, password);
        }

        // ------------------------------------------------------------------
        // SM2 加密
        // ------------------------------------------------------------------

        public static string Sm2EncryptHex(string publicKeyHex, byte[] message)
        {
            if (message == null || message.Length == 0)
            {
                throw new ArgumentException("SM2 明文不能为空", nameof(message));
            }
            byte[] pub = HexToBytes(publicKeyHex);
            if (pub.Length == 65 && pub[0] == 0x04)
            {
                byte[] xy = new byte[64];
                Buffer.BlockCopy(pub, 1, xy, 0, 64);
                pub = xy;
            }
            if (pub.Length != 64)
            {
                throw new ArgumentException("SM2 公钥长度非法");
            }

            BigInteger px = FromBe(pub, 0, 32);
            BigInteger py = FromBe(pub, 32, 32);
            Point pubPoint = new Point(px, py, BigInteger.One);
            if (!IsOnCurve(pubPoint))
            {
                throw new ArgumentException("SM2 公钥不在曲线上");
            }

            Point g = new Point(Gx, Gy, BigInteger.One);

            for (int attempt = 0; attempt < 64; attempt++)
            {
                BigInteger k = RandomScalar();
                Point c1 = ScalarMultiply(g, k);
                Point s = ScalarMultiply(pubPoint, k);
                if (s.IsInfinity)
                {
                    continue;
                }
                Point sAffine = ToAffine(s);
                byte[] x2 = ToBe(sAffine.X, 32);
                byte[] y2 = ToBe(sAffine.Y, 32);

                byte[] t = Kdf(Concat(x2, y2), message.Length);
                if (IsAllZero(t))
                {
                    continue;
                }
                byte[] c2 = new byte[message.Length];
                for (int i = 0; i < message.Length; i++)
                {
                    c2[i] = (byte)(message[i] ^ t[i]);
                }

                byte[] c3 = Sm3(Concat(x2, message, y2));
                Point c1Affine = ToAffine(c1);

                byte[] output = new byte[65 + 32 + message.Length];
                output[0] = 0x04;
                Buffer.BlockCopy(ToBe(c1Affine.X, 32), 0, output, 1, 32);
                Buffer.BlockCopy(ToBe(c1Affine.Y, 32), 0, output, 33, 32);
                Buffer.BlockCopy(c3, 0, output, 65, 32);
                Buffer.BlockCopy(c2, 0, output, 97, c2.Length);
                return BytesToHex(output);
            }
            throw new CryptographicException("SM2 加密失败：无法生成有效随机数");
        }

        private static byte[] Kdf(byte[] z, int keyLength)
        {
            byte[] output = new byte[keyLength];
            int offset = 0;
            uint counter = 1;
            byte[] block = new byte[z.Length + 4];
            Buffer.BlockCopy(z, 0, block, 0, z.Length);
            while (offset < keyLength)
            {
                block[z.Length] = (byte)(counter >> 24);
                block[z.Length + 1] = (byte)(counter >> 16);
                block[z.Length + 2] = (byte)(counter >> 8);
                block[z.Length + 3] = (byte)counter;
                byte[] digest = Sm3(block);
                int take = Math.Min(digest.Length, keyLength - offset);
                Buffer.BlockCopy(digest, 0, output, offset, take);
                offset += take;
                counter++;
            }
            return output;
        }

        // ------------------------------------------------------------------
        // 椭圆曲线（Jacobian 坐标）
        // ------------------------------------------------------------------

        private struct Point
        {
            public BigInteger X;
            public BigInteger Y;
            public BigInteger Z;

            public Point(BigInteger x, BigInteger y, BigInteger z)
            {
                X = x;
                Y = y;
                Z = z;
            }

            public bool IsInfinity
            {
                get { return Z.IsZero; }
            }

            public static Point Infinity
            {
                get { return new Point(BigInteger.Zero, BigInteger.One, BigInteger.Zero); }
            }
        }

        private static bool IsOnCurve(Point p)
        {
            if (p.IsInfinity) return false;
            BigInteger left = Mod(BigInteger.ModPow(p.Y, 2, P));
            BigInteger right = Mod(BigInteger.ModPow(p.X, 3, P) + A * p.X + B);
            return left == right;
        }

        private static Point ToAffine(Point p)
        {
            if (p.IsInfinity) return p;
            BigInteger zInv = BigInteger.ModPow(p.Z, P - 2, P);
            BigInteger zInv2 = Mod(zInv * zInv);
            BigInteger x = Mod(p.X * zInv2);
            BigInteger y = Mod(p.Y * zInv2 * zInv);
            return new Point(x, y, BigInteger.One);
        }

        private static Point Double(Point p)
        {
            if (p.IsInfinity || p.Y.IsZero) return Point.Infinity;

            BigInteger y2 = Mod(p.Y * p.Y);
            BigInteger s = Mod(4 * p.X * y2);
            BigInteger z2 = Mod(p.Z * p.Z);
            BigInteger z4 = Mod(z2 * z2);
            BigInteger m = Mod(3 * p.X * p.X + A * z4);
            BigInteger x3 = Mod(m * m - 2 * s);
            BigInteger y3 = Mod(m * (s - x3) - 8 * y2 * y2);
            BigInteger z3 = Mod(2 * p.Y * p.Z);
            return new Point(x3, y3, z3);
        }

        private static Point Add(Point p1, Point p2)
        {
            if (p1.IsInfinity) return p2;
            if (p2.IsInfinity) return p1;

            BigInteger z1z1 = Mod(p1.Z * p1.Z);
            BigInteger z2z2 = Mod(p2.Z * p2.Z);
            BigInteger u1 = Mod(p1.X * z2z2);
            BigInteger u2 = Mod(p2.X * z1z1);
            BigInteger s1 = Mod(p1.Y * p2.Z * z2z2);
            BigInteger s2 = Mod(p2.Y * p1.Z * z1z1);

            if (u1 == u2)
            {
                if (s1 != s2) return Point.Infinity;
                return Double(p1);
            }

            BigInteger h = Mod(u2 - u1);
            BigInteger r = Mod(s2 - s1);
            BigInteger h2 = Mod(h * h);
            BigInteger h3 = Mod(h2 * h);
            BigInteger u1h2 = Mod(u1 * h2);
            BigInteger x3 = Mod(r * r - h3 - 2 * u1h2);
            BigInteger y3 = Mod(r * (u1h2 - x3) - s1 * h3);
            BigInteger z3 = Mod(h * p1.Z * p2.Z);
            return new Point(x3, y3, z3);
        }

        private static Point ScalarMultiply(Point p, BigInteger k)
        {
            Point result = Point.Infinity;
            Point addend = p;
            while (k > 0)
            {
                if (!(k & BigInteger.One).IsZero)
                {
                    result = Add(result, addend);
                }
                addend = Double(addend);
                k >>= 1;
            }
            return result;
        }

        private static BigInteger RandomScalar()
        {
            byte[] buffer = new byte[32];
            using (var rng = new RNGCryptoServiceProvider())
            {
                do
                {
                    rng.GetBytes(buffer);
                }
                while (IsAllZero(buffer));
            }
            BigInteger value = FromBe(buffer, 0, 32) % (N - 1);
            return value + 1;
        }

        // ------------------------------------------------------------------
        // SM3
        // ------------------------------------------------------------------

        private static readonly uint[] Sm3Iv =
        {
            0x7380166f, 0x4914b2b9, 0x172442d7, 0xda8a0600,
            0xa96f30bc, 0x163138aa, 0xe38dee4d, 0xb0fb0e4e
        };

        public static byte[] Sm3(byte[] data)
        {
            int length = data.Length;
            long bitLength = (long)length * 8;
            int padding = (length % 64 < 56) ? (56 - length % 64) : (120 - length % 64);
            byte[] buffer = new byte[length + padding + 8];
            Buffer.BlockCopy(data, 0, buffer, 0, length);
            buffer[length] = 0x80;
            for (int i = 0; i < 8; i++)
            {
                buffer[buffer.Length - 1 - i] = (byte)(bitLength >> (8 * i));
            }

            uint[] v = (uint[])Sm3Iv.Clone();
            uint[] w = new uint[68];
            uint[] w1 = new uint[64];

            for (int offset = 0; offset < buffer.Length; offset += 64)
            {
                for (int i = 0; i < 16; i++)
                {
                    w[i] = ((uint)buffer[offset + i * 4] << 24)
                         | ((uint)buffer[offset + i * 4 + 1] << 16)
                         | ((uint)buffer[offset + i * 4 + 2] << 8)
                         | buffer[offset + i * 4 + 3];
                }
                for (int j = 16; j < 68; j++)
                {
                    w[j] = P1(w[j - 16] ^ w[j - 9] ^ Rotl(w[j - 3], 15))
                         ^ Rotl(w[j - 13], 7)
                         ^ w[j - 6];
                }
                for (int j = 0; j < 64; j++)
                {
                    w1[j] = w[j] ^ w[j + 4];
                }

                uint a = v[0], b = v[1], c = v[2], d = v[3];
                uint e = v[4], f = v[5], g = v[6], h = v[7];

                for (int j = 0; j < 64; j++)
                {
                    uint tj = j < 16 ? 0x79cc4519u : 0x7a879d8au;
                    uint ss1 = Rotl(unchecked(Rotl(a, 12) + e + Rotl(tj, j % 32)), 7);
                    uint ss2 = ss1 ^ Rotl(a, 12);
                    uint ff = j < 16 ? (a ^ b ^ c) : ((a & b) | (a & c) | (b & c));
                    uint gg = j < 16 ? (e ^ f ^ g) : ((e & f) | (~e & g));
                    uint tt1 = unchecked(ff + d + ss2 + w1[j]);
                    uint tt2 = unchecked(gg + h + ss1 + w[j]);

                    d = c;
                    c = Rotl(b, 9);
                    b = a;
                    a = tt1;
                    h = g;
                    g = Rotl(f, 19);
                    f = e;
                    e = P0(tt2);
                }

                v[0] ^= a; v[1] ^= b; v[2] ^= c; v[3] ^= d;
                v[4] ^= e; v[5] ^= f; v[6] ^= g; v[7] ^= h;
            }

            byte[] output = new byte[32];
            for (int i = 0; i < 8; i++)
            {
                output[i * 4] = (byte)(v[i] >> 24);
                output[i * 4 + 1] = (byte)(v[i] >> 16);
                output[i * 4 + 2] = (byte)(v[i] >> 8);
                output[i * 4 + 3] = (byte)v[i];
            }
            return output;
        }

        private static uint P0(uint x) { return x ^ Rotl(x, 9) ^ Rotl(x, 17); }
        private static uint P1(uint x) { return x ^ Rotl(x, 15) ^ Rotl(x, 23); }
        private static uint Rotl(uint x, int n) { return (x << n) | (x >> (32 - n)); }

        // ------------------------------------------------------------------
        // SM4-ECB/PKCS#7
        // ------------------------------------------------------------------

        private static readonly uint[] Sm4Fk =
        {
            0xa3b1bac6, 0x56aa3350, 0x677d9197, 0xb27022dc
        };

        private static readonly uint[] Sm4Ck =
        {
            0x00070e15, 0x1c232a31, 0x383f464d, 0x545b6269,
            0x70777e85, 0x8c939aa1, 0xa8afb6bd, 0xc4cbd2d9,
            0xe0e7eef5, 0xfc030a11, 0x181f262d, 0x343b4249,
            0x50575e65, 0x6c737a81, 0x888f969d, 0xa4abb2b9,
            0xc0c7ced5, 0xdce3eaf1, 0xf8ff060d, 0x141b2229,
            0x30373e45, 0x4c535a61, 0x686f767d, 0x848b9299,
            0xa0a7aeb5, 0xbcc3cad1, 0xd8dfe6ed, 0xf4fb0209,
            0x10171e25, 0x2c333a41, 0x484f565d, 0x646b7279
        };

        private static readonly byte[] Sm4Sbox =
        {
            0xd6,0x90,0xe9,0xfe,0xcc,0xe1,0x3d,0xb7,0x16,0xb6,0x14,0xc2,0x28,0xfb,0x2c,0x05,
            0x2b,0x67,0x9a,0x76,0x2a,0xbe,0x04,0xc3,0xaa,0x44,0x13,0x26,0x49,0x86,0x06,0x99,
            0x9c,0x42,0x50,0xf4,0x91,0xef,0x98,0x7a,0x33,0x54,0x0b,0x43,0xed,0xcf,0xac,0x62,
            0xe4,0xb3,0x1c,0xa9,0xc9,0x08,0xe8,0x95,0x80,0xdf,0x94,0xfa,0x75,0x8f,0x3f,0xa6,
            0x47,0x07,0xa7,0xfc,0xf3,0x73,0x17,0xba,0x83,0x59,0x3c,0x19,0xe6,0x85,0x4f,0xa8,
            0x68,0x6b,0x81,0xb2,0x71,0x64,0xda,0x8b,0xf8,0xeb,0x0f,0x4b,0x70,0x56,0x9d,0x35,
            0x1e,0x24,0x0e,0x5e,0x63,0x58,0xd1,0xa2,0x25,0x22,0x7c,0x3b,0x01,0x21,0x78,0x87,
            0xd4,0x00,0x46,0x57,0x9f,0xd3,0x27,0x52,0x4c,0x36,0x02,0xe7,0xa0,0xc4,0xc8,0x9e,
            0xea,0xbf,0x8a,0xd2,0x40,0xc7,0x38,0xb5,0xa3,0xf7,0xf2,0xce,0xf9,0x61,0x15,0xa1,
            0xe0,0xae,0x5d,0xa4,0x9b,0x34,0x1a,0x55,0xad,0x93,0x32,0x30,0xf5,0x8c,0xb1,0xe3,
            0x1d,0xf6,0xe2,0x2e,0x82,0x66,0xca,0x60,0xc0,0x29,0x23,0xab,0x0d,0x53,0x4e,0x6f,
            0xd5,0xdb,0x37,0x45,0xde,0xfd,0x8e,0x2f,0x03,0xff,0x6a,0x72,0x6d,0x6c,0x5b,0x51,
            0x8d,0x1b,0xaf,0x92,0xbb,0xdd,0xbc,0x7f,0x11,0xd9,0x5c,0x41,0x1f,0x10,0x5a,0xd8,
            0x0a,0xc1,0x31,0x88,0xa5,0xcd,0x7b,0xbd,0x2d,0x74,0xd0,0x12,0xb8,0xe5,0xb4,0xb0,
            0x89,0x69,0x97,0x4a,0x0c,0x96,0x77,0x7e,0x65,0xb9,0xf1,0x09,0xc5,0x6e,0xc6,0x84,
            0x18,0xf0,0x7d,0xec,0x3a,0xdc,0x4d,0x20,0x79,0xee,0x5f,0x3e,0xd7,0xcb,0x39,0x48
        };

        /// <summary>SM4-ECB/PKCS#7 加密；`keyHex` 为 32 位 hex，明文按 UTF-8 处理。</summary>
        public static string Sm4EcbEncryptHex(string keyHex, string plaintext)
        {
            return Sm4EcbEncryptHex(keyHex, Encoding.UTF8.GetBytes(plaintext));
        }

        /// <summary>SM4-ECB/PKCS#7 加密原始字节，返回小写 hex。</summary>
        public static string Sm4EcbEncryptHex(string keyHex, byte[] input)
        {
            byte[] key = HexToBytes(keyHex);
            if (key.Length != 16)
            {
                throw new ArgumentException("SM4 密钥必须是 16 字节", nameof(keyHex));
            }
            if (input == null)
            {
                throw new ArgumentNullException(nameof(input));
            }

            int pad = 16 - (input.Length % 16);
            byte[] buffer = new byte[input.Length + pad];
            Buffer.BlockCopy(input, 0, buffer, 0, input.Length);
            for (int i = input.Length; i < buffer.Length; i++)
            {
                buffer[i] = (byte)pad;
            }

            uint[] roundKeys = Sm4KeySchedule(key);
            for (int offset = 0; offset < buffer.Length; offset += 16)
            {
                Sm4Block(roundKeys, buffer, offset);
            }
            return BytesToHex(buffer);
        }

        private static uint[] Sm4KeySchedule(byte[] key)
        {
            uint[] k = new uint[4];
            for (int i = 0; i < 4; i++)
            {
                k[i] = ((uint)key[i * 4] << 24) | ((uint)key[i * 4 + 1] << 16)
                     | ((uint)key[i * 4 + 2] << 8) | key[i * 4 + 3];
                k[i] ^= Sm4Fk[i];
            }

            uint[] roundKeys = new uint[32];
            for (int i = 0; i < 32; i++)
            {
                uint t = k[(i + 1) % 4] ^ k[(i + 2) % 4] ^ k[(i + 3) % 4] ^ Sm4Ck[i];
                uint next = k[i % 4] ^ Sm4KeyTransform(t);
                roundKeys[i] = next;
                k[i % 4] = next;
            }
            return roundKeys;
        }

        private static uint Sm4KeyTransform(uint x)
        {
            uint b = Sm4Sub(x);
            return b ^ Rotl(b, 13) ^ Rotl(b, 23);
        }

        private static void Sm4Block(uint[] roundKeys, byte[] data, int offset)
        {
            uint x0 = ((uint)data[offset] << 24) | ((uint)data[offset + 1] << 16)
                    | ((uint)data[offset + 2] << 8) | data[offset + 3];
            uint x1 = ((uint)data[offset + 4] << 24) | ((uint)data[offset + 5] << 16)
                    | ((uint)data[offset + 6] << 8) | data[offset + 7];
            uint x2 = ((uint)data[offset + 8] << 24) | ((uint)data[offset + 9] << 16)
                    | ((uint)data[offset + 10] << 8) | data[offset + 11];
            uint x3 = ((uint)data[offset + 12] << 24) | ((uint)data[offset + 13] << 16)
                    | ((uint)data[offset + 14] << 8) | data[offset + 15];

            uint[] x = new uint[36];
            x[0] = x0; x[1] = x1; x[2] = x2; x[3] = x3;
            for (int i = 0; i < 32; i++)
            {
                uint t = x[i + 1] ^ x[i + 2] ^ x[i + 3] ^ roundKeys[i];
                uint b = Sm4Sub(t);
                uint l = b ^ Rotl(b, 2) ^ Rotl(b, 10) ^ Rotl(b, 18) ^ Rotl(b, 24);
                x[i + 4] = x[i] ^ l;
            }

            WriteUInt(data, offset, x[35]);
            WriteUInt(data, offset + 4, x[34]);
            WriteUInt(data, offset + 8, x[33]);
            WriteUInt(data, offset + 12, x[32]);
        }

        private static uint Sm4Sub(uint x)
        {
            return ((uint)Sm4Sbox[(x >> 24) & 0xff] << 24)
                 | ((uint)Sm4Sbox[(x >> 16) & 0xff] << 16)
                 | ((uint)Sm4Sbox[(x >> 8) & 0xff] << 8)
                 | Sm4Sbox[x & 0xff];
        }

        // ------------------------------------------------------------------
        // 工具
        // ------------------------------------------------------------------

        private static BigInteger Mod(BigInteger value)
        {
            BigInteger result = value % P;
            return result.Sign < 0 ? result + P : result;
        }

        private static byte[] Concat(params byte[][] parts)
        {
            int length = 0;
            foreach (byte[] part in parts) length += part.Length;
            byte[] output = new byte[length];
            int offset = 0;
            foreach (byte[] part in parts)
            {
                Buffer.BlockCopy(part, 0, output, offset, part.Length);
                offset += part.Length;
            }
            return output;
        }

        private static bool IsAllZero(byte[] data)
        {
            foreach (byte b in data) if (b != 0) return false;
            return true;
        }

        private static void WriteUInt(byte[] buffer, int offset, uint value)
        {
            buffer[offset] = (byte)(value >> 24);
            buffer[offset + 1] = (byte)(value >> 16);
            buffer[offset + 2] = (byte)(value >> 8);
            buffer[offset + 3] = (byte)value;
        }

        public static BigInteger FromHex(string hex)
        {
            return FromBe(HexToBytes(hex), 0, hex.Length / 2);
        }

        private static BigInteger FromBe(byte[] data, int offset, int length)
        {
            byte[] little = new byte[length + 1];
            for (int i = 0; i < length; i++)
            {
                little[i] = data[offset + length - 1 - i];
            }
            return new BigInteger(little);
        }

        private static byte[] ToBe(BigInteger value, int length)
        {
            byte[] little = value.ToByteArray();
            byte[] output = new byte[length];
            int count = Math.Min(length, little.Length);
            for (int i = 0; i < count; i++)
            {
                output[length - 1 - i] = little[i];
            }
            return output;
        }

        public static byte[] HexToBytes(string hex)
        {
            hex = (hex ?? string.Empty).Trim();
            if (hex.Length % 2 != 0)
            {
                throw new ArgumentException("hex 长度必须为偶数");
            }
            byte[] bytes = new byte[hex.Length / 2];
            for (int i = 0; i < bytes.Length; i++)
            {
                bytes[i] = Convert.ToByte(hex.Substring(i * 2, 2), 16);
            }
            return bytes;
        }

        public static string BytesToHex(byte[] data)
        {
            var sb = new StringBuilder(data.Length * 2);
            foreach (byte b in data)
            {
                sb.Append(b.ToString("x2"));
            }
            return sb.ToString();
        }

        private static string RandomHex(int bytes)
        {
            byte[] buffer = new byte[bytes];
            using (var rng = new RNGCryptoServiceProvider())
            {
                rng.GetBytes(buffer);
            }
            return BytesToHex(buffer);
        }
    }
}
