(() => {
  var __defProp = Object.defineProperty;
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __defNormalProp = (obj, key, value) => key in obj ? __defProp(obj, key, { enumerable: true, configurable: true, writable: true, value }) : obj[key] = value;
  var __esm = (fn, res) => function __init() {
    return fn && (res = (0, fn[__getOwnPropNames(fn)[0]])(fn = 0)), res;
  };
  var __commonJS = (cb, mod) => function __require() {
    return mod || (0, cb[__getOwnPropNames(cb)[0]])((mod = { exports: {} }).exports, mod), mod.exports;
  };
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };
  var __publicField = (obj, key, value) => __defNormalProp(obj, typeof key !== "symbol" ? key + "" : key, value);

  // node_modules/@dfinity/ic-pub-key/dist/encoding.js
  function blobEncode(bytes) {
    return [...bytes].map((p) => blobEncodeU8(p)).join("");
  }
  function blobDecode(s) {
    const ans = [];
    let skip = 0;
    let byte = 0;
    for (const char of s) {
      if (skip == 2) {
        byte = parseInt(char, 16);
        skip--;
        continue;
      }
      if (skip == 1) {
        byte = byte * 16 + parseInt(char, 16);
        ans.push(byte);
        skip--;
        continue;
      }
      if (char === "\\") {
        skip = 2;
        byte = 0;
        continue;
      }
      ans.push(char.charCodeAt(0));
    }
    if (skip > 0) {
      throw new Error("Incomplete escape sequence at the end of the input string.");
    }
    return new Uint8Array(ans);
  }
  function isAsciiAlphanumeric(code) {
    return code >= 48 && code <= 57 || // 0-9
    code >= 65 && code <= 90 || // A-Z
    code >= 97 && code <= 122;
  }
  function blobEncodeU8(u8) {
    if (isAsciiAlphanumeric(u8)) {
      return String.fromCharCode(u8);
    }
    return `\\${u8.toString(16).padStart(2, "0")}`;
  }
  function bigintFromBigEndianBytes(bytes) {
    if (bytes.length === 0) {
      return BigInt(0);
    }
    const bigEndianHex = "0x" + Buffer.from(bytes).toString("hex");
    return BigInt(bigEndianHex);
  }
  var init_encoding = __esm({
    "node_modules/@dfinity/ic-pub-key/dist/encoding.js"() {
    }
  });

  // node_modules/@dfinity/ic-pub-key/dist/chain_code.js
  var ChainCode;
  var init_chain_code = __esm({
    "node_modules/@dfinity/ic-pub-key/dist/chain_code.js"() {
      init_encoding();
      ChainCode = class _ChainCode {
        constructor(bytes) {
          this.bytes = bytes;
          if (bytes.length !== _ChainCode.LENGTH) {
            throw new Error(`Invalid ChainCode length: expected ${_ChainCode.LENGTH} bytes, got ${bytes.length}`);
          }
        }
        /**
         * Creates a new ChainCode from a 64 character hex string.
         * @param hex_str The 64 character hex string.
         * @returns A new ChainCode.
         */
        static fromHex(hex_str) {
          if (hex_str.length !== _ChainCode.LENGTH * 2) {
            throw new Error(`Invalid ChainCode length: expected ${_ChainCode.LENGTH * 2} characters, got ${hex_str.length}`);
          }
          const bytes = Buffer.from(hex_str, "hex");
          return new _ChainCode(new Uint8Array(bytes));
        }
        static fromArray(array) {
          return new _ChainCode(new Uint8Array(array));
        }
        /**
         * @returns The chain code as a 64 character hex string.
         */
        toHex() {
          return Buffer.from(this.bytes).toString("hex");
        }
        /**
         * Creates a new ChainCode from a Candid blob.
         * @param blob The blob to create the chain code from.
         * @returns A new ChainCode.
         */
        static fromBlob(blob) {
          return new _ChainCode(blobDecode(blob));
        }
        /**
         * @returns The chain code as a Candid blob.
         */
        toBlob() {
          return blobEncode(this.bytes);
        }
        /**
         * Creates a new ChainCode from a string.
         * @param str The chain code as a hex string or Candid blob.
         * @returns A new ChainCode.
         */
        static fromString(str) {
          if (str.length === _ChainCode.LENGTH * 2 && str.match(/^[0-9A-Fa-f]+$/)) {
            return _ChainCode.fromHex(str);
          }
          return _ChainCode.fromBlob(str);
        }
        toJSON() {
          return this.toHex();
        }
      };
      ChainCode.LENGTH = 32;
    }
  });

  // node_modules/@noble/hashes/esm/utils.js
  function isBytes(a) {
    return a instanceof Uint8Array || ArrayBuffer.isView(a) && a.constructor.name === "Uint8Array";
  }
  function anumber(n) {
    if (!Number.isSafeInteger(n) || n < 0)
      throw new Error("positive integer expected, got " + n);
  }
  function abytes(b, ...lengths) {
    if (!isBytes(b))
      throw new Error("Uint8Array expected");
    if (lengths.length > 0 && !lengths.includes(b.length))
      throw new Error("Uint8Array expected of length " + lengths + ", got length=" + b.length);
  }
  function ahash(h2) {
    if (typeof h2 !== "function" || typeof h2.create !== "function")
      throw new Error("Hash should be wrapped by utils.createHasher");
    anumber(h2.outputLen);
    anumber(h2.blockLen);
  }
  function aexists(instance, checkFinished = true) {
    if (instance.destroyed)
      throw new Error("Hash instance has been destroyed");
    if (checkFinished && instance.finished)
      throw new Error("Hash#digest() has already been called");
  }
  function aoutput(out, instance) {
    abytes(out);
    const min = instance.outputLen;
    if (out.length < min) {
      throw new Error("digestInto() expects output buffer of length at least " + min);
    }
  }
  function clean(...arrays) {
    for (let i = 0; i < arrays.length; i++) {
      arrays[i].fill(0);
    }
  }
  function createView(arr) {
    return new DataView(arr.buffer, arr.byteOffset, arr.byteLength);
  }
  function bytesToHex(bytes) {
    abytes(bytes);
    if (hasHexBuiltin)
      return bytes.toHex();
    let hex = "";
    for (let i = 0; i < bytes.length; i++) {
      hex += hexes[bytes[i]];
    }
    return hex;
  }
  function utf8ToBytes(str) {
    if (typeof str !== "string")
      throw new Error("string expected");
    return new Uint8Array(new TextEncoder().encode(str));
  }
  function toBytes(data) {
    if (typeof data === "string")
      data = utf8ToBytes(data);
    abytes(data);
    return data;
  }
  function createHasher(hashCons) {
    const hashC = (msg) => hashCons().update(toBytes(msg)).digest();
    const tmp = hashCons();
    hashC.outputLen = tmp.outputLen;
    hashC.blockLen = tmp.blockLen;
    hashC.create = () => hashCons();
    return hashC;
  }
  var hasHexBuiltin, hexes, Hash;
  var init_utils = __esm({
    "node_modules/@noble/hashes/esm/utils.js"() {
      hasHexBuiltin = /* @__PURE__ */ (() => (
        // @ts-ignore
        typeof Uint8Array.from([]).toHex === "function" && typeof Uint8Array.fromHex === "function"
      ))();
      hexes = /* @__PURE__ */ Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, "0"));
      Hash = class {
      };
    }
  });

  // node_modules/@noble/hashes/esm/hmac.js
  var HMAC, hmac;
  var init_hmac = __esm({
    "node_modules/@noble/hashes/esm/hmac.js"() {
      init_utils();
      HMAC = class extends Hash {
        constructor(hash, _key) {
          super();
          this.finished = false;
          this.destroyed = false;
          ahash(hash);
          const key = toBytes(_key);
          this.iHash = hash.create();
          if (typeof this.iHash.update !== "function")
            throw new Error("Expected instance of class which extends utils.Hash");
          this.blockLen = this.iHash.blockLen;
          this.outputLen = this.iHash.outputLen;
          const blockLen = this.blockLen;
          const pad = new Uint8Array(blockLen);
          pad.set(key.length > blockLen ? hash.create().update(key).digest() : key);
          for (let i = 0; i < pad.length; i++)
            pad[i] ^= 54;
          this.iHash.update(pad);
          this.oHash = hash.create();
          for (let i = 0; i < pad.length; i++)
            pad[i] ^= 54 ^ 92;
          this.oHash.update(pad);
          clean(pad);
        }
        update(buf) {
          aexists(this);
          this.iHash.update(buf);
          return this;
        }
        digestInto(out) {
          aexists(this);
          abytes(out, this.outputLen);
          this.finished = true;
          this.iHash.digestInto(out);
          this.oHash.update(out);
          this.oHash.digestInto(out);
          this.destroy();
        }
        digest() {
          const out = new Uint8Array(this.oHash.outputLen);
          this.digestInto(out);
          return out;
        }
        _cloneInto(to) {
          to || (to = Object.create(Object.getPrototypeOf(this), {}));
          const { oHash, iHash, finished, destroyed, blockLen, outputLen } = this;
          to = to;
          to.finished = finished;
          to.destroyed = destroyed;
          to.blockLen = blockLen;
          to.outputLen = outputLen;
          to.oHash = oHash._cloneInto(to.oHash);
          to.iHash = iHash._cloneInto(to.iHash);
          return to;
        }
        clone() {
          return this._cloneInto();
        }
        destroy() {
          this.destroyed = true;
          this.oHash.destroy();
          this.iHash.destroy();
        }
      };
      hmac = (hash, key, message) => new HMAC(hash, key).update(message).digest();
      hmac.create = (hash, key) => new HMAC(hash, key);
    }
  });

  // node_modules/@noble/hashes/esm/_md.js
  function setBigUint64(view, byteOffset, value, isLE) {
    if (typeof view.setBigUint64 === "function")
      return view.setBigUint64(byteOffset, value, isLE);
    const _32n2 = BigInt(32);
    const _u32_max = BigInt(4294967295);
    const wh = Number(value >> _32n2 & _u32_max);
    const wl = Number(value & _u32_max);
    const h2 = isLE ? 4 : 0;
    const l = isLE ? 0 : 4;
    view.setUint32(byteOffset + h2, wh, isLE);
    view.setUint32(byteOffset + l, wl, isLE);
  }
  var HashMD, SHA512_IV;
  var init_md = __esm({
    "node_modules/@noble/hashes/esm/_md.js"() {
      init_utils();
      HashMD = class extends Hash {
        constructor(blockLen, outputLen, padOffset, isLE) {
          super();
          this.finished = false;
          this.length = 0;
          this.pos = 0;
          this.destroyed = false;
          this.blockLen = blockLen;
          this.outputLen = outputLen;
          this.padOffset = padOffset;
          this.isLE = isLE;
          this.buffer = new Uint8Array(blockLen);
          this.view = createView(this.buffer);
        }
        update(data) {
          aexists(this);
          data = toBytes(data);
          abytes(data);
          const { view, buffer, blockLen } = this;
          const len = data.length;
          for (let pos = 0; pos < len; ) {
            const take = Math.min(blockLen - this.pos, len - pos);
            if (take === blockLen) {
              const dataView = createView(data);
              for (; blockLen <= len - pos; pos += blockLen)
                this.process(dataView, pos);
              continue;
            }
            buffer.set(data.subarray(pos, pos + take), this.pos);
            this.pos += take;
            pos += take;
            if (this.pos === blockLen) {
              this.process(view, 0);
              this.pos = 0;
            }
          }
          this.length += data.length;
          this.roundClean();
          return this;
        }
        digestInto(out) {
          aexists(this);
          aoutput(out, this);
          this.finished = true;
          const { buffer, view, blockLen, isLE } = this;
          let { pos } = this;
          buffer[pos++] = 128;
          clean(this.buffer.subarray(pos));
          if (this.padOffset > blockLen - pos) {
            this.process(view, 0);
            pos = 0;
          }
          for (let i = pos; i < blockLen; i++)
            buffer[i] = 0;
          setBigUint64(view, blockLen - 8, BigInt(this.length * 8), isLE);
          this.process(view, 0);
          const oview = createView(out);
          const len = this.outputLen;
          if (len % 4)
            throw new Error("_sha2: outputLen should be aligned to 32bit");
          const outLen = len / 4;
          const state = this.get();
          if (outLen > state.length)
            throw new Error("_sha2: outputLen bigger than state");
          for (let i = 0; i < outLen; i++)
            oview.setUint32(4 * i, state[i], isLE);
        }
        digest() {
          const { buffer, outputLen } = this;
          this.digestInto(buffer);
          const res = buffer.slice(0, outputLen);
          this.destroy();
          return res;
        }
        _cloneInto(to) {
          to || (to = new this.constructor());
          to.set(...this.get());
          const { blockLen, buffer, length, finished, destroyed, pos } = this;
          to.destroyed = destroyed;
          to.finished = finished;
          to.length = length;
          to.pos = pos;
          if (length % blockLen)
            to.buffer.set(buffer);
          return to;
        }
        clone() {
          return this._cloneInto();
        }
      };
      SHA512_IV = /* @__PURE__ */ Uint32Array.from([
        1779033703,
        4089235720,
        3144134277,
        2227873595,
        1013904242,
        4271175723,
        2773480762,
        1595750129,
        1359893119,
        2917565137,
        2600822924,
        725511199,
        528734635,
        4215389547,
        1541459225,
        327033209
      ]);
    }
  });

  // node_modules/@noble/hashes/esm/_u64.js
  function fromBig(n, le = false) {
    if (le)
      return { h: Number(n & U32_MASK64), l: Number(n >> _32n & U32_MASK64) };
    return { h: Number(n >> _32n & U32_MASK64) | 0, l: Number(n & U32_MASK64) | 0 };
  }
  function split(lst, le = false) {
    const len = lst.length;
    let Ah = new Uint32Array(len);
    let Al = new Uint32Array(len);
    for (let i = 0; i < len; i++) {
      const { h: h2, l } = fromBig(lst[i], le);
      [Ah[i], Al[i]] = [h2, l];
    }
    return [Ah, Al];
  }
  function add(Ah, Al, Bh, Bl) {
    const l = (Al >>> 0) + (Bl >>> 0);
    return { h: Ah + Bh + (l / 2 ** 32 | 0) | 0, l: l | 0 };
  }
  var U32_MASK64, _32n, shrSH, shrSL, rotrSH, rotrSL, rotrBH, rotrBL, add3L, add3H, add4L, add4H, add5L, add5H;
  var init_u64 = __esm({
    "node_modules/@noble/hashes/esm/_u64.js"() {
      U32_MASK64 = /* @__PURE__ */ BigInt(2 ** 32 - 1);
      _32n = /* @__PURE__ */ BigInt(32);
      shrSH = (h2, _l, s) => h2 >>> s;
      shrSL = (h2, l, s) => h2 << 32 - s | l >>> s;
      rotrSH = (h2, l, s) => h2 >>> s | l << 32 - s;
      rotrSL = (h2, l, s) => h2 << 32 - s | l >>> s;
      rotrBH = (h2, l, s) => h2 << 64 - s | l >>> s - 32;
      rotrBL = (h2, l, s) => h2 >>> s - 32 | l << 64 - s;
      add3L = (Al, Bl, Cl) => (Al >>> 0) + (Bl >>> 0) + (Cl >>> 0);
      add3H = (low, Ah, Bh, Ch) => Ah + Bh + Ch + (low / 2 ** 32 | 0) | 0;
      add4L = (Al, Bl, Cl, Dl) => (Al >>> 0) + (Bl >>> 0) + (Cl >>> 0) + (Dl >>> 0);
      add4H = (low, Ah, Bh, Ch, Dh) => Ah + Bh + Ch + Dh + (low / 2 ** 32 | 0) | 0;
      add5L = (Al, Bl, Cl, Dl, El) => (Al >>> 0) + (Bl >>> 0) + (Cl >>> 0) + (Dl >>> 0) + (El >>> 0);
      add5H = (low, Ah, Bh, Ch, Dh, Eh) => Ah + Bh + Ch + Dh + Eh + (low / 2 ** 32 | 0) | 0;
    }
  });

  // node_modules/@noble/hashes/esm/sha2.js
  var K512, SHA512_Kh, SHA512_Kl, SHA512_W_H, SHA512_W_L, SHA512, sha512;
  var init_sha2 = __esm({
    "node_modules/@noble/hashes/esm/sha2.js"() {
      init_md();
      init_u64();
      init_utils();
      K512 = /* @__PURE__ */ (() => split([
        "0x428a2f98d728ae22",
        "0x7137449123ef65cd",
        "0xb5c0fbcfec4d3b2f",
        "0xe9b5dba58189dbbc",
        "0x3956c25bf348b538",
        "0x59f111f1b605d019",
        "0x923f82a4af194f9b",
        "0xab1c5ed5da6d8118",
        "0xd807aa98a3030242",
        "0x12835b0145706fbe",
        "0x243185be4ee4b28c",
        "0x550c7dc3d5ffb4e2",
        "0x72be5d74f27b896f",
        "0x80deb1fe3b1696b1",
        "0x9bdc06a725c71235",
        "0xc19bf174cf692694",
        "0xe49b69c19ef14ad2",
        "0xefbe4786384f25e3",
        "0x0fc19dc68b8cd5b5",
        "0x240ca1cc77ac9c65",
        "0x2de92c6f592b0275",
        "0x4a7484aa6ea6e483",
        "0x5cb0a9dcbd41fbd4",
        "0x76f988da831153b5",
        "0x983e5152ee66dfab",
        "0xa831c66d2db43210",
        "0xb00327c898fb213f",
        "0xbf597fc7beef0ee4",
        "0xc6e00bf33da88fc2",
        "0xd5a79147930aa725",
        "0x06ca6351e003826f",
        "0x142929670a0e6e70",
        "0x27b70a8546d22ffc",
        "0x2e1b21385c26c926",
        "0x4d2c6dfc5ac42aed",
        "0x53380d139d95b3df",
        "0x650a73548baf63de",
        "0x766a0abb3c77b2a8",
        "0x81c2c92e47edaee6",
        "0x92722c851482353b",
        "0xa2bfe8a14cf10364",
        "0xa81a664bbc423001",
        "0xc24b8b70d0f89791",
        "0xc76c51a30654be30",
        "0xd192e819d6ef5218",
        "0xd69906245565a910",
        "0xf40e35855771202a",
        "0x106aa07032bbd1b8",
        "0x19a4c116b8d2d0c8",
        "0x1e376c085141ab53",
        "0x2748774cdf8eeb99",
        "0x34b0bcb5e19b48a8",
        "0x391c0cb3c5c95a63",
        "0x4ed8aa4ae3418acb",
        "0x5b9cca4f7763e373",
        "0x682e6ff3d6b2b8a3",
        "0x748f82ee5defb2fc",
        "0x78a5636f43172f60",
        "0x84c87814a1f0ab72",
        "0x8cc702081a6439ec",
        "0x90befffa23631e28",
        "0xa4506cebde82bde9",
        "0xbef9a3f7b2c67915",
        "0xc67178f2e372532b",
        "0xca273eceea26619c",
        "0xd186b8c721c0c207",
        "0xeada7dd6cde0eb1e",
        "0xf57d4f7fee6ed178",
        "0x06f067aa72176fba",
        "0x0a637dc5a2c898a6",
        "0x113f9804bef90dae",
        "0x1b710b35131c471b",
        "0x28db77f523047d84",
        "0x32caab7b40c72493",
        "0x3c9ebe0a15c9bebc",
        "0x431d67c49c100d4c",
        "0x4cc5d4becb3e42b6",
        "0x597f299cfc657e2a",
        "0x5fcb6fab3ad6faec",
        "0x6c44198c4a475817"
      ].map((n) => BigInt(n))))();
      SHA512_Kh = /* @__PURE__ */ (() => K512[0])();
      SHA512_Kl = /* @__PURE__ */ (() => K512[1])();
      SHA512_W_H = /* @__PURE__ */ new Uint32Array(80);
      SHA512_W_L = /* @__PURE__ */ new Uint32Array(80);
      SHA512 = class extends HashMD {
        constructor(outputLen = 64) {
          super(128, outputLen, 16, false);
          this.Ah = SHA512_IV[0] | 0;
          this.Al = SHA512_IV[1] | 0;
          this.Bh = SHA512_IV[2] | 0;
          this.Bl = SHA512_IV[3] | 0;
          this.Ch = SHA512_IV[4] | 0;
          this.Cl = SHA512_IV[5] | 0;
          this.Dh = SHA512_IV[6] | 0;
          this.Dl = SHA512_IV[7] | 0;
          this.Eh = SHA512_IV[8] | 0;
          this.El = SHA512_IV[9] | 0;
          this.Fh = SHA512_IV[10] | 0;
          this.Fl = SHA512_IV[11] | 0;
          this.Gh = SHA512_IV[12] | 0;
          this.Gl = SHA512_IV[13] | 0;
          this.Hh = SHA512_IV[14] | 0;
          this.Hl = SHA512_IV[15] | 0;
        }
        // prettier-ignore
        get() {
          const { Ah, Al, Bh, Bl, Ch, Cl, Dh, Dl, Eh, El, Fh, Fl, Gh, Gl, Hh, Hl } = this;
          return [Ah, Al, Bh, Bl, Ch, Cl, Dh, Dl, Eh, El, Fh, Fl, Gh, Gl, Hh, Hl];
        }
        // prettier-ignore
        set(Ah, Al, Bh, Bl, Ch, Cl, Dh, Dl, Eh, El, Fh, Fl, Gh, Gl, Hh, Hl) {
          this.Ah = Ah | 0;
          this.Al = Al | 0;
          this.Bh = Bh | 0;
          this.Bl = Bl | 0;
          this.Ch = Ch | 0;
          this.Cl = Cl | 0;
          this.Dh = Dh | 0;
          this.Dl = Dl | 0;
          this.Eh = Eh | 0;
          this.El = El | 0;
          this.Fh = Fh | 0;
          this.Fl = Fl | 0;
          this.Gh = Gh | 0;
          this.Gl = Gl | 0;
          this.Hh = Hh | 0;
          this.Hl = Hl | 0;
        }
        process(view, offset) {
          for (let i = 0; i < 16; i++, offset += 4) {
            SHA512_W_H[i] = view.getUint32(offset);
            SHA512_W_L[i] = view.getUint32(offset += 4);
          }
          for (let i = 16; i < 80; i++) {
            const W15h = SHA512_W_H[i - 15] | 0;
            const W15l = SHA512_W_L[i - 15] | 0;
            const s0h = rotrSH(W15h, W15l, 1) ^ rotrSH(W15h, W15l, 8) ^ shrSH(W15h, W15l, 7);
            const s0l = rotrSL(W15h, W15l, 1) ^ rotrSL(W15h, W15l, 8) ^ shrSL(W15h, W15l, 7);
            const W2h = SHA512_W_H[i - 2] | 0;
            const W2l = SHA512_W_L[i - 2] | 0;
            const s1h = rotrSH(W2h, W2l, 19) ^ rotrBH(W2h, W2l, 61) ^ shrSH(W2h, W2l, 6);
            const s1l = rotrSL(W2h, W2l, 19) ^ rotrBL(W2h, W2l, 61) ^ shrSL(W2h, W2l, 6);
            const SUMl = add4L(s0l, s1l, SHA512_W_L[i - 7], SHA512_W_L[i - 16]);
            const SUMh = add4H(SUMl, s0h, s1h, SHA512_W_H[i - 7], SHA512_W_H[i - 16]);
            SHA512_W_H[i] = SUMh | 0;
            SHA512_W_L[i] = SUMl | 0;
          }
          let { Ah, Al, Bh, Bl, Ch, Cl, Dh, Dl, Eh, El, Fh, Fl, Gh, Gl, Hh, Hl } = this;
          for (let i = 0; i < 80; i++) {
            const sigma1h = rotrSH(Eh, El, 14) ^ rotrSH(Eh, El, 18) ^ rotrBH(Eh, El, 41);
            const sigma1l = rotrSL(Eh, El, 14) ^ rotrSL(Eh, El, 18) ^ rotrBL(Eh, El, 41);
            const CHIh = Eh & Fh ^ ~Eh & Gh;
            const CHIl = El & Fl ^ ~El & Gl;
            const T1ll = add5L(Hl, sigma1l, CHIl, SHA512_Kl[i], SHA512_W_L[i]);
            const T1h = add5H(T1ll, Hh, sigma1h, CHIh, SHA512_Kh[i], SHA512_W_H[i]);
            const T1l = T1ll | 0;
            const sigma0h = rotrSH(Ah, Al, 28) ^ rotrBH(Ah, Al, 34) ^ rotrBH(Ah, Al, 39);
            const sigma0l = rotrSL(Ah, Al, 28) ^ rotrBL(Ah, Al, 34) ^ rotrBL(Ah, Al, 39);
            const MAJh = Ah & Bh ^ Ah & Ch ^ Bh & Ch;
            const MAJl = Al & Bl ^ Al & Cl ^ Bl & Cl;
            Hh = Gh | 0;
            Hl = Gl | 0;
            Gh = Fh | 0;
            Gl = Fl | 0;
            Fh = Eh | 0;
            Fl = El | 0;
            ({ h: Eh, l: El } = add(Dh | 0, Dl | 0, T1h | 0, T1l | 0));
            Dh = Ch | 0;
            Dl = Cl | 0;
            Ch = Bh | 0;
            Cl = Bl | 0;
            Bh = Ah | 0;
            Bl = Al | 0;
            const All = add3L(T1l, sigma0l, MAJl);
            Ah = add3H(All, T1h, sigma0h, MAJh);
            Al = All | 0;
          }
          ({ h: Ah, l: Al } = add(this.Ah | 0, this.Al | 0, Ah | 0, Al | 0));
          ({ h: Bh, l: Bl } = add(this.Bh | 0, this.Bl | 0, Bh | 0, Bl | 0));
          ({ h: Ch, l: Cl } = add(this.Ch | 0, this.Cl | 0, Ch | 0, Cl | 0));
          ({ h: Dh, l: Dl } = add(this.Dh | 0, this.Dl | 0, Dh | 0, Dl | 0));
          ({ h: Eh, l: El } = add(this.Eh | 0, this.El | 0, Eh | 0, El | 0));
          ({ h: Fh, l: Fl } = add(this.Fh | 0, this.Fl | 0, Fh | 0, Fl | 0));
          ({ h: Gh, l: Gl } = add(this.Gh | 0, this.Gl | 0, Gh | 0, Gl | 0));
          ({ h: Hh, l: Hl } = add(this.Hh | 0, this.Hl | 0, Hh | 0, Hl | 0));
          this.set(Ah, Al, Bh, Bl, Ch, Cl, Dh, Dl, Eh, El, Fh, Fl, Gh, Gl, Hh, Hl);
        }
        roundClean() {
          clean(SHA512_W_H, SHA512_W_L);
        }
        destroy() {
          clean(this.buffer);
          this.set(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        }
      };
      sha512 = /* @__PURE__ */ createHasher(() => new SHA512());
    }
  });

  // node_modules/@noble/secp256k1/index.js
  var secp256k1_CURVE, P, N, Gx, Gy, _b, L, L2, err, isBig, isStr, isBytes2, abytes2, u8n, u8fr, padh, bytesToHex2, C, _ch, hexToBytes, toU8, concatBytes, big, arange, M, invert, apoint, koblitz, afield0, afield, agroup, isEven, u8of, getPrefix, lift_x, _Point, Point, G, I, bytesToNumBE, sliceBytesNumBE, B256, numTo32b, toPrivScalar, W, scalarBits, pwindows, pwindowSize, precompute, Gpows, ctneg, wNAF;
  var init_secp256k1 = __esm({
    "node_modules/@noble/secp256k1/index.js"() {
      secp256k1_CURVE = {
        p: 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2fn,
        n: 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n,
        h: 1n,
        a: 0n,
        b: 7n,
        Gx: 0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798n,
        Gy: 0x483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8n
      };
      ({ p: P, n: N, Gx, Gy, b: _b } = secp256k1_CURVE);
      L = 32;
      L2 = 64;
      err = (m = "") => {
        throw new Error(m);
      };
      isBig = (n) => typeof n === "bigint";
      isStr = (s) => typeof s === "string";
      isBytes2 = (a) => a instanceof Uint8Array || ArrayBuffer.isView(a) && a.constructor.name === "Uint8Array";
      abytes2 = (a, l) => !isBytes2(a) || typeof l === "number" && l > 0 && a.length !== l ? err("Uint8Array expected") : a;
      u8n = (len) => new Uint8Array(len);
      u8fr = (buf) => Uint8Array.from(buf);
      padh = (n, pad) => n.toString(16).padStart(pad, "0");
      bytesToHex2 = (b) => Array.from(abytes2(b)).map((e) => padh(e, 2)).join("");
      C = { _0: 48, _9: 57, A: 65, F: 70, a: 97, f: 102 };
      _ch = (ch) => {
        if (ch >= C._0 && ch <= C._9)
          return ch - C._0;
        if (ch >= C.A && ch <= C.F)
          return ch - (C.A - 10);
        if (ch >= C.a && ch <= C.f)
          return ch - (C.a - 10);
        return;
      };
      hexToBytes = (hex) => {
        const e = "hex invalid";
        if (!isStr(hex))
          return err(e);
        const hl = hex.length;
        const al = hl / 2;
        if (hl % 2)
          return err(e);
        const array = u8n(al);
        for (let ai = 0, hi = 0; ai < al; ai++, hi += 2) {
          const n1 = _ch(hex.charCodeAt(hi));
          const n2 = _ch(hex.charCodeAt(hi + 1));
          if (n1 === void 0 || n2 === void 0)
            return err(e);
          array[ai] = n1 * 16 + n2;
        }
        return array;
      };
      toU8 = (a, len) => abytes2(isStr(a) ? hexToBytes(a) : u8fr(abytes2(a)), len);
      concatBytes = (...arrs) => {
        const r = u8n(arrs.reduce((sum, a) => sum + abytes2(a).length, 0));
        let pad = 0;
        arrs.forEach((a) => {
          r.set(a, pad);
          pad += a.length;
        });
        return r;
      };
      big = BigInt;
      arange = (n, min, max, msg = "bad number: out of range") => isBig(n) && min <= n && n < max ? n : err(msg);
      M = (a, b = P) => {
        const r = a % b;
        return r >= 0n ? r : b + r;
      };
      invert = (num, md) => {
        if (num === 0n || md <= 0n)
          err("no inverse n=" + num + " mod=" + md);
        let a = M(num, md), b = md, x = 0n, y = 1n, u = 1n, v = 0n;
        while (a !== 0n) {
          const q = b / a, r = b % a;
          const m = x - u * q, n = y - v * q;
          b = a, a = r, x = u, y = v, u = m, v = n;
        }
        return b === 1n ? M(x, md) : err("no inverse");
      };
      apoint = (p) => p instanceof Point ? p : err("Point expected");
      koblitz = (x) => M(M(x * x) * x + _b);
      afield0 = (n) => arange(n, 0n, P);
      afield = (n) => arange(n, 1n, P);
      agroup = (n) => arange(n, 1n, N);
      isEven = (y) => (y & 1n) === 0n;
      u8of = (n) => Uint8Array.of(n);
      getPrefix = (y) => u8of(isEven(y) ? 2 : 3);
      lift_x = (x) => {
        const c = koblitz(afield(x));
        let r = 1n;
        for (let num = c, e = (P + 1n) / 4n; e > 0n; e >>= 1n) {
          if (e & 1n)
            r = r * num % P;
          num = num * num % P;
        }
        return M(r * r) === c ? r : err("sqrt invalid");
      };
      _Point = class _Point {
        constructor(px, py, pz) {
          __publicField(this, "px");
          __publicField(this, "py");
          __publicField(this, "pz");
          this.px = afield0(px);
          this.py = afield(py);
          this.pz = afield0(pz);
          Object.freeze(this);
        }
        /** Convert Uint8Array or hex string to Point. */
        static fromBytes(bytes) {
          abytes2(bytes);
          let p = void 0;
          const head = bytes[0];
          const tail = bytes.subarray(1);
          const x = sliceBytesNumBE(tail, 0, L);
          const len = bytes.length;
          if (len === L + 1 && [2, 3].includes(head)) {
            let y = lift_x(x);
            const evenY = isEven(y);
            const evenH = isEven(big(head));
            if (evenH !== evenY)
              y = M(-y);
            p = new _Point(x, y, 1n);
          }
          if (len === L2 + 1 && head === 4)
            p = new _Point(x, sliceBytesNumBE(tail, L, L2), 1n);
          return p ? p.assertValidity() : err("bad point: not on curve");
        }
        /** Equality check: compare points P&Q. */
        equals(other) {
          const { px: X1, py: Y1, pz: Z1 } = this;
          const { px: X2, py: Y2, pz: Z2 } = apoint(other);
          const X1Z2 = M(X1 * Z2);
          const X2Z1 = M(X2 * Z1);
          const Y1Z2 = M(Y1 * Z2);
          const Y2Z1 = M(Y2 * Z1);
          return X1Z2 === X2Z1 && Y1Z2 === Y2Z1;
        }
        is0() {
          return this.equals(I);
        }
        /** Flip point over y coordinate. */
        negate() {
          return new _Point(this.px, M(-this.py), this.pz);
        }
        /** Point doubling: P+P, complete formula. */
        double() {
          return this.add(this);
        }
        /**
         * Point addition: P+Q, complete, exception-free formula
         * (Renes-Costello-Batina, algo 1 of [2015/1060](https://eprint.iacr.org/2015/1060)).
         * Cost: `12M + 0S + 3*a + 3*b3 + 23add`.
         */
        // prettier-ignore
        add(other) {
          const { px: X1, py: Y1, pz: Z1 } = this;
          const { px: X2, py: Y2, pz: Z2 } = apoint(other);
          const a = 0n;
          const b = _b;
          let X3 = 0n, Y3 = 0n, Z3 = 0n;
          const b3 = M(b * 3n);
          let t0 = M(X1 * X2), t1 = M(Y1 * Y2), t2 = M(Z1 * Z2), t3 = M(X1 + Y1);
          let t4 = M(X2 + Y2);
          t3 = M(t3 * t4);
          t4 = M(t0 + t1);
          t3 = M(t3 - t4);
          t4 = M(X1 + Z1);
          let t5 = M(X2 + Z2);
          t4 = M(t4 * t5);
          t5 = M(t0 + t2);
          t4 = M(t4 - t5);
          t5 = M(Y1 + Z1);
          X3 = M(Y2 + Z2);
          t5 = M(t5 * X3);
          X3 = M(t1 + t2);
          t5 = M(t5 - X3);
          Z3 = M(a * t4);
          X3 = M(b3 * t2);
          Z3 = M(X3 + Z3);
          X3 = M(t1 - Z3);
          Z3 = M(t1 + Z3);
          Y3 = M(X3 * Z3);
          t1 = M(t0 + t0);
          t1 = M(t1 + t0);
          t2 = M(a * t2);
          t4 = M(b3 * t4);
          t1 = M(t1 + t2);
          t2 = M(t0 - t2);
          t2 = M(a * t2);
          t4 = M(t4 + t2);
          t0 = M(t1 * t4);
          Y3 = M(Y3 + t0);
          t0 = M(t5 * t4);
          X3 = M(t3 * X3);
          X3 = M(X3 - t0);
          t0 = M(t3 * t1);
          Z3 = M(t5 * Z3);
          Z3 = M(Z3 + t0);
          return new _Point(X3, Y3, Z3);
        }
        /**
         * Point-by-scalar multiplication. Scalar must be in range 1 <= n < CURVE.n.
         * Uses {@link wNAF} for base point.
         * Uses fake point to mitigate side-channel leakage.
         * @param n scalar by which point is multiplied
         * @param safe safe mode guards against timing attacks; unsafe mode is faster
         */
        multiply(n, safe = true) {
          if (!safe && n === 0n)
            return I;
          agroup(n);
          if (n === 1n)
            return this;
          if (this.equals(G))
            return wNAF(n).p;
          let p = I;
          let f = G;
          for (let d = this; n > 0n; d = d.double(), n >>= 1n) {
            if (n & 1n)
              p = p.add(d);
            else if (safe)
              f = f.add(d);
          }
          return p;
        }
        /** Convert point to 2d xy affine point. (X, Y, Z) ∋ (x=X/Z, y=Y/Z) */
        toAffine() {
          const { px: x, py: y, pz: z } = this;
          if (this.equals(I))
            return { x: 0n, y: 0n };
          if (z === 1n)
            return { x, y };
          const iz = invert(z, P);
          if (M(z * iz) !== 1n)
            err("inverse invalid");
          return { x: M(x * iz), y: M(y * iz) };
        }
        /** Checks if the point is valid and on-curve. */
        assertValidity() {
          const { x, y } = this.toAffine();
          afield(x);
          afield(y);
          return M(y * y) === koblitz(x) ? this : err("bad point: not on curve");
        }
        /** Converts point to 33/65-byte Uint8Array. */
        toBytes(isCompressed = true) {
          const { x, y } = this.assertValidity().toAffine();
          const x32b = numTo32b(x);
          if (isCompressed)
            return concatBytes(getPrefix(y), x32b);
          return concatBytes(u8of(4), x32b, numTo32b(y));
        }
        /** Create 3d xyz point from 2d xy. (0, 0) => (0, 1, 0), not (0, 0, 1) */
        static fromAffine(ap) {
          const { x, y } = ap;
          return x === 0n && y === 0n ? I : new _Point(x, y, 1n);
        }
        toHex(isCompressed) {
          return bytesToHex2(this.toBytes(isCompressed));
        }
        static fromPrivateKey(k) {
          return G.multiply(toPrivScalar(k));
        }
        static fromHex(hex) {
          return _Point.fromBytes(toU8(hex));
        }
        get x() {
          return this.toAffine().x;
        }
        get y() {
          return this.toAffine().y;
        }
        toRawBytes(isCompressed) {
          return this.toBytes(isCompressed);
        }
      };
      __publicField(_Point, "BASE");
      __publicField(_Point, "ZERO");
      Point = _Point;
      G = new Point(Gx, Gy, 1n);
      I = new Point(0n, 1n, 0n);
      Point.BASE = G;
      Point.ZERO = I;
      bytesToNumBE = (b) => big("0x" + (bytesToHex2(b) || "0"));
      sliceBytesNumBE = (b, from, to) => bytesToNumBE(b.subarray(from, to));
      B256 = 2n ** 256n;
      numTo32b = (num) => hexToBytes(padh(arange(num, 0n, B256), L2));
      toPrivScalar = (pr) => {
        const num = isBig(pr) ? pr : bytesToNumBE(toU8(pr, L));
        return arange(num, 1n, N, "private key invalid 3");
      };
      W = 8;
      scalarBits = 256;
      pwindows = Math.ceil(scalarBits / W) + 1;
      pwindowSize = 2 ** (W - 1);
      precompute = () => {
        const points = [];
        let p = G;
        let b = p;
        for (let w = 0; w < pwindows; w++) {
          b = p;
          points.push(b);
          for (let i = 1; i < pwindowSize; i++) {
            b = b.add(p);
            points.push(b);
          }
          p = b.double();
        }
        return points;
      };
      Gpows = void 0;
      ctneg = (cnd, p) => {
        const n = p.negate();
        return cnd ? n : p;
      };
      wNAF = (n) => {
        const comp = Gpows || (Gpows = precompute());
        let p = I;
        let f = G;
        const pow_2_w = 2 ** W;
        const maxNum = pow_2_w;
        const mask = big(pow_2_w - 1);
        const shiftBy = big(W);
        for (let w = 0; w < pwindows; w++) {
          let wbits = Number(n & mask);
          n >>= shiftBy;
          if (wbits > pwindowSize) {
            wbits -= maxNum;
            n += 1n;
          }
          const off = w * pwindowSize;
          const offF = off;
          const offP = off + Math.abs(wbits) - 1;
          const isEven2 = w % 2 !== 0;
          const isNeg = wbits < 0;
          if (wbits === 0) {
            f = f.add(ctneg(isEven2, comp[offF]));
          } else {
            p = p.add(ctneg(isNeg, comp[offP]));
          }
        }
        return { p, f };
      };
    }
  });

  // node_modules/@dfinity/ic-pub-key/dist/ecdsa/secp256k1.js
  var PublicKeyWithChainCode, Sec1EncodedPublicKey, DerivationPath;
  var init_secp256k12 = __esm({
    "node_modules/@dfinity/ic-pub-key/dist/ecdsa/secp256k1.js"() {
      init_hmac();
      init_sha2();
      init_utils();
      init_secp256k1();
      init_chain_code();
      init_encoding();
      PublicKeyWithChainCode = class _PublicKeyWithChainCode {
        /**
         * @param public_key The public key.
         * @param chain_code A hash of the derivation path.
         */
        constructor(public_key, chain_code) {
          this.public_key = public_key;
          this.chain_code = chain_code;
        }
        /**
         * Return the master public key used in the production mainnet
         */
        static forMainnetKey(key_id) {
          const chain_key = ChainCode.fromHex("0000000000000000000000000000000000000000000000000000000000000000");
          if (key_id === "key_1") {
            const public_key = Sec1EncodedPublicKey.fromHex("02121bc3a5c38f38ca76487c72007ebbfd34bc6c4cb80a671655aa94585bbd0a02");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else if (key_id === "test_key_1") {
            const public_key = Sec1EncodedPublicKey.fromHex("02f9ac345f6be6db51e1c5612cddb59e72c3d0d493c994d12035cf13257e3b1fa7");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else {
            throw new Error("Unknown master public key id");
          }
        }
        /**
         * Return the master public key used by PocketIC for testing
         */
        static forPocketIcKey(key_id) {
          const chain_key = ChainCode.fromHex("0000000000000000000000000000000000000000000000000000000000000000");
          if (key_id === "key_1") {
            const public_key = Sec1EncodedPublicKey.fromHex("036ad6e838b46811ad79c37b2f4b854b7a05f406715b2935edc5d3251e7666977b");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else if (key_id === "test_key_1") {
            const public_key = Sec1EncodedPublicKey.fromHex("03cc365e15cb552589c7175717b2ac63d1050b9bb2e5aed35432b1b1be55d3abcf");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else if (key_id === "dfx_test_key") {
            const public_key = Sec1EncodedPublicKey.fromHex("03e6f78b1a90e361c5cc9903f73bb8acbe3bc17ad01e82554d25cf0ecd70c67484");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else {
            throw new Error("Unknown master public key id");
          }
        }
        /**
         * Creates a new PublicKeyWithChainCode from two hex strings.
         * @param public_key The public key as a 66 character hex string.
         * @param chain_code The chain code as a 64 character hex string.
         * @returns A new PublicKeyWithChainCode.
         */
        static fromHex({ public_key, chain_code }) {
          return new _PublicKeyWithChainCode(Sec1EncodedPublicKey.fromHex(public_key), ChainCode.fromHex(chain_code));
        }
        /**
         * @returns The public key and chain code as hex strings.
         */
        toHex() {
          return { public_key: this.public_key.toHex(), chain_code: this.chain_code.toHex() };
        }
        /**
         * @returns The public key and chain code as Candid blobs.
         */
        toBlob() {
          return { public_key: this.public_key.toBlob(), chain_code: this.chain_code.toBlob() };
        }
        /**
         * Creates a new PublicKeyWithChainCode from two Candid blobs.
         * @param public_key The public key as a Candid blob.
         * @param chain_code The chain code as a Candid blob.
         * @returns A new PublicKeyWithChainCode.
         */
        static fromBlob({ public_key, chain_code }) {
          return new _PublicKeyWithChainCode(Sec1EncodedPublicKey.fromBlob(public_key), ChainCode.fromBlob(chain_code));
        }
        /**
         * Creates a new PublicKeyWithChainCode from two strings.
         * @param public_key The public key in any supported format.
         * @param chain_code The chain code in any supported format.
         * @returns A new PublicKeyWithChainCode.
         */
        static fromString({ public_key, chain_code }) {
          return new _PublicKeyWithChainCode(Sec1EncodedPublicKey.fromString(public_key), ChainCode.fromString(chain_code));
        }
        /**
         * Applies the given derivation path to obtain a new public key and chain code.
         */
        deriveSubkeyWithChainCode(derivation_path) {
          return this.public_key.deriveSubkeyWithChainCode(derivation_path, this.chain_code);
        }
      };
      Sec1EncodedPublicKey = class _Sec1EncodedPublicKey {
        /**
         * @param bytes The 33 sec1 bytes of the public key.
         */
        constructor(bytes) {
          this.bytes = bytes;
          if (bytes.length !== _Sec1EncodedPublicKey.LENGTH) {
            throw new Error(`Invalid PublicKey length: expected ${_Sec1EncodedPublicKey.LENGTH} bytes, got ${bytes.length}`);
          }
        }
        toAffinePoint() {
          return Point.fromHex(this.bytes).toAffine();
        }
        static fromProjectivePoint(point) {
          return new _Sec1EncodedPublicKey(point.toRawBytes(true));
        }
        /**
         * A typescript translation of [ic_secp256k1::PublicKey::derive_subkey_with_chain_code](https://github.com/dfinity/ic/blob/bb6e758c739768ef6713f9f3be2df47884544900/packages/ic-secp256k1/src/lib.rs#L678)
         * @param derivation_path The derivation path to derive the subkey from.
         * @returns A tuple containing the derived subkey and the chain code.
         */
        deriveSubkeyWithChainCode(derivation_path, chain_code) {
          const public_key = this.toAffinePoint();
          const [affine_pt, _offset, new_chain_code] = derivation_path.derive_offset(public_key, chain_code);
          const pt = Point.fromAffine(affine_pt);
          return new PublicKeyWithChainCode(_Sec1EncodedPublicKey.fromProjectivePoint(pt), new_chain_code);
        }
        /**
         * @returns The public key as a Buffer.
         */
        toBuffer() {
          return Buffer.from(this.bytes);
        }
        /**
         * Creates a new Sec1EncodedPublicKey from a 66 character hex string.
         * @param hex The 66 character hex string.
         * @returns A new Sec1EncodedPublicKey.
         */
        static fromHex(hex) {
          if (hex.length !== _Sec1EncodedPublicKey.LENGTH * 2) {
            throw new Error(`Invalid PublicKey length: expected ${_Sec1EncodedPublicKey.LENGTH * 2} characters, got ${hex.length}`);
          }
          const bytes = Buffer.from(hex, "hex");
          return new _Sec1EncodedPublicKey(new Uint8Array(bytes));
        }
        /**
         * @returns The public key as a 66-character hex string.
         */
        toHex() {
          return this.toBuffer().toString("hex");
        }
        /**
         * Creates a new Sec1EncodedPublicKey from a Candid blob.
         * @param blob The blob to create the public key from.
         * @returns A new Sec1EncodedPublicKey.
         */
        static fromBlob(blob) {
          return new _Sec1EncodedPublicKey(blobDecode(blob));
        }
        /**
         * @returns The public key as a Candid blob.
         */
        toBlob() {
          return blobEncode(this.bytes);
        }
        /**
         * Creates a new Sec1EncodedPublicKey from a string.
         * @param str The string to create the public key from.
         * @returns A new Sec1EncodedPublicKey.
         */
        static fromString(str) {
          if (str.length === _Sec1EncodedPublicKey.LENGTH * 2 && str.match(/^[0-9A-Fa-f]+$/)) {
            return _Sec1EncodedPublicKey.fromHex(str);
          }
          return _Sec1EncodedPublicKey.fromBlob(str);
        }
      };
      Sec1EncodedPublicKey.LENGTH = 33;
      DerivationPath = class _DerivationPath {
        constructor(path) {
          this.path = path;
        }
        /**
         * Creates a new DerivationPath from a canister identifier plus other path components
         * @param canisterId the id of the canister
         * @param path other path components
         */
        static withCanisterPrefix(canisterId, path) {
          const canisterIdBytes = canisterId.toUint8Array();
          const newPath = [canisterIdBytes, ...path];
          return new _DerivationPath(newPath);
        }
        /**
         * Creates a new DerivationPath from / separated candid blobs.
         * @param blob The / separated blobs to create the derivation path from.
         * @returns A new DerivationPath.
         */
        static fromBlob(blob) {
          if (blob === void 0 || blob === null) {
            return new _DerivationPath([]);
          }
          return new _DerivationPath(blob.split("/").map((p) => blobDecode(p)));
        }
        /**
         * @returns A string representation of the derivation path: Candid blob encoded with a '/' between each path component.
         */
        toBlob() {
          if (this.path.length === 0) {
            return null;
          }
          return this.path.map((p) => blobEncode(p)).join("/");
        }
        /**
         * A typescript translation of [ic_secp256k1::DerivationPath::derive_offset](https://github.com/dfinity/ic/blob/bb6e758c739768ef6713f9f3be2df47884544900/packages/ic-secp256k1/src/lib.rs#L168)
         * @param pt The public key to derive the offset from.
         * @param chain_code The chain code to derive the offset from.
         * @returns A tuple containing the derived public key, the offset, and the chain code.
         *
         * Properties:
         * - The public key is not ProjectivePoint.ZERO.
         * - The offset is strictly less than DerivationPath.ORDER.
         */
        derive_offset(pt, chain_code) {
          return this.path.reduce(([pt2, offset, chain_code2], idx) => {
            const [next_chain_code, next_offset, next_pt] = _DerivationPath.ckd_pub(idx, pt2, chain_code2);
            offset += next_offset;
            while (offset >= _DerivationPath.ORDER) {
              offset -= _DerivationPath.ORDER;
            }
            return [next_pt, offset, next_chain_code];
          }, [pt, 0n, chain_code]);
        }
        /**
         * A typescript translation of [ic_secp256k1::DerivationPath::ckd_pub](https://github.com/dfinity/ic/blob/bb6e758c739768ef6713f9f3be2df47884544900/packages/ic-secp256k1/src/lib.rs#L138)
         * @param idx A part of the derivation path.
         * @param pt The public key to derive the offset from.
         * @param chain_code The chain code to derive the offset from.
         * @returns A tuple containing the derived chain code, the offset, and the derived public key.
         *
         * Properties:
         * - The offset is strictly less than DerivationPath.ORDER.
         * - The public key is not ProjectivePoint.ZERO.
         */
        static ckd_pub(idx, pt, chain_code) {
          const ckd_input = Point.fromAffine(pt).toRawBytes(true);
          while (true) {
            const [next_chain_code, next_offset] = _DerivationPath.ckd(idx, ckd_input, chain_code);
            const base_mul = Point.BASE.multiply(next_offset);
            const next_pt = Point.fromAffine(pt).add(base_mul);
            if (!next_pt.equals(Point.ZERO)) {
              return [next_chain_code, next_offset, next_pt.toAffine()];
            }
            ckd_input[0] = 1;
            ckd_input.set(next_chain_code.bytes, 1);
          }
        }
        /**
         * A typescript translation of [ic_secp256k1::DerivationPath::ckd](https://github.com/dfinity/ic/blob/bb6e758c739768ef6713f9f3be2df47884544900/packages/ic-secp256k1/src/lib.rs#L111)
         * @param idx A part of the derivation path.
         * @param ckd_input The input to derive the offset from.
         * @param chain_code The chain code to derive the offset from.
         * @returns A tuple containing the derived chain code and the offset.
         *
         * Properties:
         * - The offset is strictly less than DerivationPath.ORDER.
         */
        static ckd(idx, ckd_input, chain_code) {
          const message = new Uint8Array(ckd_input.length + idx.length);
          message.set(ckd_input);
          message.set(idx, ckd_input.length);
          const hmac_output = hmac(sha512, chain_code.bytes, message);
          if (hmac_output.length !== 64) {
            throw new Error("Invalid HMAC output length");
          }
          const fb = hmac_output.subarray(0, 32);
          const fb_hex = bytesToHex(fb);
          const next_chain_key = hmac_output.subarray(32, 64);
          const next_offset = BigInt(`0x${fb_hex}`);
          if (next_offset >= _DerivationPath.ORDER) {
            const next_input = new Uint8Array(33);
            next_input[0] = 1;
            next_input.set(next_chain_key, 1);
            return _DerivationPath.ckd(idx, next_input, chain_code);
          }
          const next_chain_key_array = new Uint8Array(next_chain_key);
          return [new ChainCode(next_chain_key_array), next_offset];
        }
      };
      DerivationPath.ORDER = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n;
    }
  });

  // node_modules/@dfinity/ic-pub-key/dist/ecdsa/index.js
  var init_ecdsa = __esm({
    "node_modules/@dfinity/ic-pub-key/dist/ecdsa/index.js"() {
      init_secp256k12();
    }
  });

  // node_modules/@dfinity/ic-pub-key/dist/schnorr/bip340secp256k1.js
  var bip340secp256k1_exports = {};
  __export(bip340secp256k1_exports, {
    ChainCode: () => ChainCode,
    DerivationPath: () => DerivationPath,
    PublicKeyWithChainCode: () => PublicKeyWithChainCode2,
    Sec1EncodedPublicKey: () => Sec1EncodedPublicKey
  });
  var PublicKeyWithChainCode2;
  var init_bip340secp256k1 = __esm({
    "node_modules/@dfinity/ic-pub-key/dist/schnorr/bip340secp256k1.js"() {
      init_chain_code();
      init_secp256k12();
      PublicKeyWithChainCode2 = class _PublicKeyWithChainCode {
        /**
         * @param public_key The public key.
         * @param chain_code A hash of the derivation path.
         */
        constructor(public_key, chain_code) {
          this.public_key = public_key;
          this.chain_code = chain_code;
        }
        /**
         * Return the master public key used in the production mainnet
         */
        static forMainnetKey(key_id) {
          const chain_key = ChainCode.fromHex("0000000000000000000000000000000000000000000000000000000000000000");
          if (key_id === "key_1") {
            const public_key = Sec1EncodedPublicKey.fromHex("02246e29785f06d37a8a50c49f6152a34df74738f8c13a44f59fef4cbe90eb13ac");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else if (key_id === "test_key_1") {
            const public_key = Sec1EncodedPublicKey.fromHex("037a651a2e5ef3d1ef63e84c4c4caa029fa4a43a347a91e4d84a8e846853d51be1");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else {
            throw new Error("Unknown master public key id");
          }
        }
        /**
         * Return the master public key used by PocketIC for testing
         */
        static forPocketIcKey(key_id) {
          const chain_key = ChainCode.fromHex("0000000000000000000000000000000000000000000000000000000000000000");
          if (key_id === "key_1") {
            const public_key = Sec1EncodedPublicKey.fromHex("036ad6e838b46811ad79c37b2f4b854b7a05f406715b2935edc5d3251e7666977b");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else if (key_id === "test_key_1") {
            const public_key = Sec1EncodedPublicKey.fromHex("03cc365e15cb552589c7175717b2ac63d1050b9bb2e5aed35432b1b1be55d3abcf");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else if (key_id === "dfx_test_key") {
            const public_key = Sec1EncodedPublicKey.fromHex("03e6f78b1a90e361c5cc9903f73bb8acbe3bc17ad01e82554d25cf0ecd70c67484");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else {
            throw new Error("Unknown master public key id");
          }
        }
        /**
         * Creates a new PublicKeyWithChainCode from two hex strings.
         * @param public_key The public key as a 66 character hex string.
         * @param chain_code The chain code as a 64 character hex string.
         * @returns A new PublicKeyWithChainCode.
         */
        static fromHex({ public_key, chain_code }) {
          return new _PublicKeyWithChainCode(Sec1EncodedPublicKey.fromHex(public_key), ChainCode.fromHex(chain_code));
        }
        /**
         * @returns The public key and chain code as hex strings.
         */
        toHex() {
          return { public_key: this.public_key.toHex(), chain_code: this.chain_code.toHex() };
        }
        /**
         * @returns The public key and chain code as Candid blobs.
         */
        toBlob() {
          return { public_key: this.public_key.toBlob(), chain_code: this.chain_code.toBlob() };
        }
        /**
         * Creates a new PublicKeyWithChainCode from two Candid blobs.
         * @param public_key The public key as a Candid blob.
         * @param chain_code The chain code as a Candid blob.
         * @returns A new PublicKeyWithChainCode.
         */
        static fromBlob({ public_key, chain_code }) {
          return new _PublicKeyWithChainCode(Sec1EncodedPublicKey.fromBlob(public_key), ChainCode.fromBlob(chain_code));
        }
        /**
         * Creates a new PublicKeyWithChainCode from two strings.
         * @param public_key The public key in any supported format.
         * @param chain_code The chain code in any supported format.
         * @returns A new PublicKeyWithChainCode.
         */
        static fromString({ public_key, chain_code }) {
          return new _PublicKeyWithChainCode(Sec1EncodedPublicKey.fromString(public_key), ChainCode.fromString(chain_code));
        }
        /**
         * Applies the given derivation path to obtain a new public key and chain code.
         */
        deriveSubkeyWithChainCode(derivation_path) {
          return this.public_key.deriveSubkeyWithChainCode(derivation_path, this.chain_code);
        }
      };
    }
  });

  // node_modules/@noble/ed25519/index.js
  var ed25519_CURVE, P2, N2, Gx2, Gy2, _a, _d, h, L3, L22, err2, isBig2, isStr2, isBytes3, abytes3, u8n2, u8fr2, padh2, bytesToHex3, C2, _ch2, hexToBytes2, toU82, big2, arange2, M2, invert2, apoint2, B2562, _Point2, Point2, G2, I2, numTo32bLE, bytesToNumLE, pow2, pow_2_252_3, RM1, uvRatio, W2, scalarBits2, pwindows2, pwindowSize2, precompute2, Gpows2, ctneg2, wNAF2;
  var init_ed25519 = __esm({
    "node_modules/@noble/ed25519/index.js"() {
      ed25519_CURVE = {
        p: 0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffedn,
        n: 0x1000000000000000000000000000000014def9dea2f79cd65812631a5cf5d3edn,
        h: 8n,
        a: 0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffecn,
        d: 0x52036cee2b6ffe738cc740797779e89800700a4d4141d8ab75eb4dca135978a3n,
        Gx: 0x216936d3cd6e53fec0a4e231fdd6dc5c692cc7609525a7b2c9562d608f25d51an,
        Gy: 0x6666666666666666666666666666666666666666666666666666666666666658n
      };
      ({ p: P2, n: N2, Gx: Gx2, Gy: Gy2, a: _a, d: _d } = ed25519_CURVE);
      h = 8n;
      L3 = 32;
      L22 = 64;
      err2 = (m = "") => {
        throw new Error(m);
      };
      isBig2 = (n) => typeof n === "bigint";
      isStr2 = (s) => typeof s === "string";
      isBytes3 = (a) => a instanceof Uint8Array || ArrayBuffer.isView(a) && a.constructor.name === "Uint8Array";
      abytes3 = (a, l) => !isBytes3(a) || typeof l === "number" && l > 0 && a.length !== l ? err2("Uint8Array expected") : a;
      u8n2 = (len) => new Uint8Array(len);
      u8fr2 = (buf) => Uint8Array.from(buf);
      padh2 = (n, pad) => n.toString(16).padStart(pad, "0");
      bytesToHex3 = (b) => Array.from(abytes3(b)).map((e) => padh2(e, 2)).join("");
      C2 = { _0: 48, _9: 57, A: 65, F: 70, a: 97, f: 102 };
      _ch2 = (ch) => {
        if (ch >= C2._0 && ch <= C2._9)
          return ch - C2._0;
        if (ch >= C2.A && ch <= C2.F)
          return ch - (C2.A - 10);
        if (ch >= C2.a && ch <= C2.f)
          return ch - (C2.a - 10);
        return;
      };
      hexToBytes2 = (hex) => {
        const e = "hex invalid";
        if (!isStr2(hex))
          return err2(e);
        const hl = hex.length;
        const al = hl / 2;
        if (hl % 2)
          return err2(e);
        const array = u8n2(al);
        for (let ai = 0, hi = 0; ai < al; ai++, hi += 2) {
          const n1 = _ch2(hex.charCodeAt(hi));
          const n2 = _ch2(hex.charCodeAt(hi + 1));
          if (n1 === void 0 || n2 === void 0)
            return err2(e);
          array[ai] = n1 * 16 + n2;
        }
        return array;
      };
      toU82 = (a, len) => abytes3(isStr2(a) ? hexToBytes2(a) : u8fr2(abytes3(a)), len);
      big2 = BigInt;
      arange2 = (n, min, max, msg = "bad number: out of range") => isBig2(n) && min <= n && n < max ? n : err2(msg);
      M2 = (a, b = P2) => {
        const r = a % b;
        return r >= 0n ? r : b + r;
      };
      invert2 = (num, md) => {
        if (num === 0n || md <= 0n)
          err2("no inverse n=" + num + " mod=" + md);
        let a = M2(num, md), b = md, x = 0n, y = 1n, u = 1n, v = 0n;
        while (a !== 0n) {
          const q = b / a, r = b % a;
          const m = x - u * q, n = y - v * q;
          b = a, a = r, x = u, y = v, u = m, v = n;
        }
        return b === 1n ? M2(x, md) : err2("no inverse");
      };
      apoint2 = (p) => p instanceof Point2 ? p : err2("Point expected");
      B2562 = 2n ** 256n;
      _Point2 = class _Point2 {
        constructor(ex, ey, ez, et) {
          __publicField(this, "ex");
          __publicField(this, "ey");
          __publicField(this, "ez");
          __publicField(this, "et");
          const max = B2562;
          this.ex = arange2(ex, 0n, max);
          this.ey = arange2(ey, 0n, max);
          this.ez = arange2(ez, 1n, max);
          this.et = arange2(et, 0n, max);
          Object.freeze(this);
        }
        static fromAffine(p) {
          return new _Point2(p.x, p.y, 1n, M2(p.x * p.y));
        }
        /** RFC8032 5.1.3: Uint8Array to Point. */
        static fromBytes(hex, zip215 = false) {
          const d = _d;
          const normed = u8fr2(abytes3(hex, L3));
          const lastByte = hex[31];
          normed[31] = lastByte & ~128;
          const y = bytesToNumLE(normed);
          const max = zip215 ? B2562 : P2;
          arange2(y, 0n, max);
          const y2 = M2(y * y);
          const u = M2(y2 - 1n);
          const v = M2(d * y2 + 1n);
          let { isValid, value: x } = uvRatio(u, v);
          if (!isValid)
            err2("bad point: y not sqrt");
          const isXOdd = (x & 1n) === 1n;
          const isLastByteOdd = (lastByte & 128) !== 0;
          if (!zip215 && x === 0n && isLastByteOdd)
            err2("bad point: x==0, isLastByteOdd");
          if (isLastByteOdd !== isXOdd)
            x = M2(-x);
          return new _Point2(x, y, 1n, M2(x * y));
        }
        /** Checks if the point is valid and on-curve. */
        assertValidity() {
          const a = _a;
          const d = _d;
          const p = this;
          if (p.is0())
            throw new Error("bad point: ZERO");
          const { ex: X, ey: Y, ez: Z, et: T } = p;
          const X2 = M2(X * X);
          const Y2 = M2(Y * Y);
          const Z2 = M2(Z * Z);
          const Z4 = M2(Z2 * Z2);
          const aX2 = M2(X2 * a);
          const left = M2(Z2 * M2(aX2 + Y2));
          const right = M2(Z4 + M2(d * M2(X2 * Y2)));
          if (left !== right)
            throw new Error("bad point: equation left != right (1)");
          const XY = M2(X * Y);
          const ZT = M2(Z * T);
          if (XY !== ZT)
            throw new Error("bad point: equation left != right (2)");
          return this;
        }
        /** Equality check: compare points P&Q. */
        equals(other) {
          const { ex: X1, ey: Y1, ez: Z1 } = this;
          const { ex: X2, ey: Y2, ez: Z2 } = apoint2(other);
          const X1Z2 = M2(X1 * Z2);
          const X2Z1 = M2(X2 * Z1);
          const Y1Z2 = M2(Y1 * Z2);
          const Y2Z1 = M2(Y2 * Z1);
          return X1Z2 === X2Z1 && Y1Z2 === Y2Z1;
        }
        is0() {
          return this.equals(I2);
        }
        /** Flip point over y coordinate. */
        negate() {
          return new _Point2(M2(-this.ex), this.ey, this.ez, M2(-this.et));
        }
        /** Point doubling. Complete formula. Cost: `4M + 4S + 1*a + 6add + 1*2`. */
        double() {
          const { ex: X1, ey: Y1, ez: Z1 } = this;
          const a = _a;
          const A = M2(X1 * X1);
          const B = M2(Y1 * Y1);
          const C3 = M2(2n * M2(Z1 * Z1));
          const D = M2(a * A);
          const x1y1 = X1 + Y1;
          const E = M2(M2(x1y1 * x1y1) - A - B);
          const G3 = D + B;
          const F = G3 - C3;
          const H = D - B;
          const X3 = M2(E * F);
          const Y3 = M2(G3 * H);
          const T3 = M2(E * H);
          const Z3 = M2(F * G3);
          return new _Point2(X3, Y3, Z3, T3);
        }
        /** Point addition. Complete formula. Cost: `8M + 1*k + 8add + 1*2`. */
        add(other) {
          const { ex: X1, ey: Y1, ez: Z1, et: T1 } = this;
          const { ex: X2, ey: Y2, ez: Z2, et: T2 } = apoint2(other);
          const a = _a;
          const d = _d;
          const A = M2(X1 * X2);
          const B = M2(Y1 * Y2);
          const C3 = M2(T1 * d * T2);
          const D = M2(Z1 * Z2);
          const E = M2((X1 + Y1) * (X2 + Y2) - A - B);
          const F = M2(D - C3);
          const G3 = M2(D + C3);
          const H = M2(B - a * A);
          const X3 = M2(E * F);
          const Y3 = M2(G3 * H);
          const T3 = M2(E * H);
          const Z3 = M2(F * G3);
          return new _Point2(X3, Y3, Z3, T3);
        }
        /**
         * Point-by-scalar multiplication. Scalar must be in range 1 <= n < CURVE.n.
         * Uses {@link wNAF} for base point.
         * Uses fake point to mitigate side-channel leakage.
         * @param n scalar by which point is multiplied
         * @param safe safe mode guards against timing attacks; unsafe mode is faster
         */
        multiply(n, safe = true) {
          if (!safe && (n === 0n || this.is0()))
            return I2;
          arange2(n, 1n, N2);
          if (n === 1n)
            return this;
          if (this.equals(G2))
            return wNAF2(n).p;
          let p = I2;
          let f = G2;
          for (let d = this; n > 0n; d = d.double(), n >>= 1n) {
            if (n & 1n)
              p = p.add(d);
            else if (safe)
              f = f.add(d);
          }
          return p;
        }
        /** Convert point to 2d xy affine point. (X, Y, Z) ∋ (x=X/Z, y=Y/Z) */
        toAffine() {
          const { ex: x, ey: y, ez: z } = this;
          if (this.equals(I2))
            return { x: 0n, y: 1n };
          const iz = invert2(z, P2);
          if (M2(z * iz) !== 1n)
            err2("invalid inverse");
          return { x: M2(x * iz), y: M2(y * iz) };
        }
        toBytes() {
          const { x, y } = this.assertValidity().toAffine();
          const b = numTo32bLE(y);
          b[31] |= x & 1n ? 128 : 0;
          return b;
        }
        toHex() {
          return bytesToHex3(this.toBytes());
        }
        // encode to hex string
        clearCofactor() {
          return this.multiply(big2(h), false);
        }
        isSmallOrder() {
          return this.clearCofactor().is0();
        }
        isTorsionFree() {
          let p = this.multiply(N2 / 2n, false).double();
          if (N2 % 2n)
            p = p.add(this);
          return p.is0();
        }
        static fromHex(hex, zip215) {
          return _Point2.fromBytes(toU82(hex), zip215);
        }
        get x() {
          return this.toAffine().x;
        }
        get y() {
          return this.toAffine().y;
        }
        toRawBytes() {
          return this.toBytes();
        }
      };
      __publicField(_Point2, "BASE");
      __publicField(_Point2, "ZERO");
      Point2 = _Point2;
      G2 = new Point2(Gx2, Gy2, 1n, M2(Gx2 * Gy2));
      I2 = new Point2(0n, 1n, 1n, 0n);
      Point2.BASE = G2;
      Point2.ZERO = I2;
      numTo32bLE = (num) => hexToBytes2(padh2(arange2(num, 0n, B2562), L22)).reverse();
      bytesToNumLE = (b) => big2("0x" + bytesToHex3(u8fr2(abytes3(b)).reverse()));
      pow2 = (x, power) => {
        let r = x;
        while (power-- > 0n) {
          r *= r;
          r %= P2;
        }
        return r;
      };
      pow_2_252_3 = (x) => {
        const x2 = x * x % P2;
        const b2 = x2 * x % P2;
        const b4 = pow2(b2, 2n) * b2 % P2;
        const b5 = pow2(b4, 1n) * x % P2;
        const b10 = pow2(b5, 5n) * b5 % P2;
        const b20 = pow2(b10, 10n) * b10 % P2;
        const b40 = pow2(b20, 20n) * b20 % P2;
        const b80 = pow2(b40, 40n) * b40 % P2;
        const b160 = pow2(b80, 80n) * b80 % P2;
        const b240 = pow2(b160, 80n) * b80 % P2;
        const b250 = pow2(b240, 10n) * b10 % P2;
        const pow_p_5_8 = pow2(b250, 2n) * x % P2;
        return { pow_p_5_8, b2 };
      };
      RM1 = 0x2b8324804fc1df0b2b4d00993dfbd7a72f431806ad2fe478c4ee1b274a0ea0b0n;
      uvRatio = (u, v) => {
        const v3 = M2(v * v * v);
        const v7 = M2(v3 * v3 * v);
        const pow = pow_2_252_3(u * v7).pow_p_5_8;
        let x = M2(u * v3 * pow);
        const vx2 = M2(v * x * x);
        const root1 = x;
        const root2 = M2(x * RM1);
        const useRoot1 = vx2 === u;
        const useRoot2 = vx2 === M2(-u);
        const noRoot = vx2 === M2(-u * RM1);
        if (useRoot1)
          x = root1;
        if (useRoot2 || noRoot)
          x = root2;
        if ((M2(x) & 1n) === 1n)
          x = M2(-x);
        return { isValid: useRoot1 || useRoot2, value: x };
      };
      W2 = 8;
      scalarBits2 = 256;
      pwindows2 = Math.ceil(scalarBits2 / W2) + 1;
      pwindowSize2 = 2 ** (W2 - 1);
      precompute2 = () => {
        const points = [];
        let p = G2;
        let b = p;
        for (let w = 0; w < pwindows2; w++) {
          b = p;
          points.push(b);
          for (let i = 1; i < pwindowSize2; i++) {
            b = b.add(p);
            points.push(b);
          }
          p = b.double();
        }
        return points;
      };
      Gpows2 = void 0;
      ctneg2 = (cnd, p) => {
        const n = p.negate();
        return cnd ? n : p;
      };
      wNAF2 = (n) => {
        const comp = Gpows2 || (Gpows2 = precompute2());
        let p = I2;
        let f = G2;
        const pow_2_w = 2 ** W2;
        const maxNum = pow_2_w;
        const mask = big2(pow_2_w - 1);
        const shiftBy = big2(W2);
        for (let w = 0; w < pwindows2; w++) {
          let wbits = Number(n & mask);
          n >>= shiftBy;
          if (wbits > pwindowSize2) {
            wbits -= maxNum;
            n += 1n;
          }
          const off = w * pwindowSize2;
          const offF = off;
          const offP = off + Math.abs(wbits) - 1;
          const isEven2 = w % 2 !== 0;
          const isNeg = wbits < 0;
          if (wbits === 0) {
            f = f.add(ctneg2(isEven2, comp[offF]));
          } else {
            p = p.add(ctneg2(isNeg, comp[offP]));
          }
        }
        return { p, f };
      };
    }
  });

  // node_modules/@noble/hashes/esm/hkdf.js
  function extract(hash, ikm, salt) {
    ahash(hash);
    if (salt === void 0)
      salt = new Uint8Array(hash.outputLen);
    return hmac(hash, toBytes(salt), toBytes(ikm));
  }
  function expand(hash, prk, info, length = 32) {
    ahash(hash);
    anumber(length);
    const olen = hash.outputLen;
    if (length > 255 * olen)
      throw new Error("Length should be <= 255*HashLen");
    const blocks = Math.ceil(length / olen);
    if (info === void 0)
      info = EMPTY_BUFFER;
    const okm = new Uint8Array(blocks * olen);
    const HMAC2 = hmac.create(hash, prk);
    const HMACTmp = HMAC2._cloneInto();
    const T = new Uint8Array(HMAC2.outputLen);
    for (let counter = 0; counter < blocks; counter++) {
      HKDF_COUNTER[0] = counter + 1;
      HMACTmp.update(counter === 0 ? EMPTY_BUFFER : T).update(info).update(HKDF_COUNTER).digestInto(T);
      okm.set(T, olen * counter);
      HMAC2._cloneInto(HMACTmp);
    }
    HMAC2.destroy();
    HMACTmp.destroy();
    clean(T, HKDF_COUNTER);
    return okm.slice(0, length);
  }
  var HKDF_COUNTER, EMPTY_BUFFER, hkdf;
  var init_hkdf = __esm({
    "node_modules/@noble/hashes/esm/hkdf.js"() {
      init_hmac();
      init_utils();
      HKDF_COUNTER = /* @__PURE__ */ Uint8Array.from([0]);
      EMPTY_BUFFER = /* @__PURE__ */ Uint8Array.of();
      hkdf = (hash, ikm, salt, info, length) => expand(hash, extract(hash, ikm, salt), info, length);
    }
  });

  // node_modules/@dfinity/ic-pub-key/dist/schnorr/ed25519.js
  var ed25519_exports = {};
  __export(ed25519_exports, {
    ChainCode: () => ChainCode,
    DerivationPath: () => DerivationPath2,
    PublicKey: () => PublicKey,
    PublicKeyWithChainCode: () => PublicKeyWithChainCode3,
    deriveOneOffset: () => deriveOneOffset,
    offsetFromOkm: () => offsetFromOkm,
    schnorrEd25519Derive: () => schnorrEd25519Derive
  });
  function deriveOneOffset([pt, sum, chainCode], idx) {
    const ptBytes = pt.toRawBytes();
    const ikm = new Uint8Array(ptBytes.length + idx.length);
    ikm.set(ptBytes, 0);
    ikm.set(idx, ptBytes.length);
    const okm = hkdf(sha512, ikm, chainCode.bytes, "Ed25519", 96);
    const offset = offsetFromOkm(okm);
    pt = pt.add(Point2.BASE.multiply(offset));
    sum = (sum + offset) % ORDER;
    chainCode = new ChainCode(okm.subarray(64, 96));
    return [pt, sum, chainCode];
  }
  function offsetFromOkm(okm) {
    const offsetBytes = new Uint8Array(okm.subarray(0, 64));
    const offset = bigintFromBigEndianBytes(offsetBytes);
    const reduced = offset % ORDER;
    return reduced;
  }
  function schnorrEd25519Derive(pubkey, chaincode, derivationPath) {
    const publicKeyWithChainCode = PublicKeyWithChainCode3.fromString(pubkey, chaincode);
    const parsedDerivationPath = DerivationPath2.fromBlob(derivationPath);
    const derivedPubkey = publicKeyWithChainCode.deriveSubkeyWithChainCode(parsedDerivationPath);
    return {
      request: {
        key: publicKeyWithChainCode,
        derivation_path: parsedDerivationPath
      },
      response: derivedPubkey
    };
  }
  var ORDER, PublicKey, PublicKeyWithChainCode3, DerivationPath2;
  var init_ed255192 = __esm({
    "node_modules/@dfinity/ic-pub-key/dist/schnorr/ed25519.js"() {
      init_ed25519();
      init_hkdf();
      init_sha2();
      init_chain_code();
      init_encoding();
      ORDER = 2n ** 252n + 27742317777372353535851937790883648493n;
      PublicKey = class _PublicKey {
        constructor(key) {
          this.key = key;
          if (key.is0()) {
            throw new Error("Invalid public key");
          }
        }
        /**
         * Parses a public key from a string in any supported format.
         */
        static fromString(public_key_string) {
          return _PublicKey.fromHex(public_key_string);
        }
        /**
         * Creates a new PublicKey from a hex string.
         * @param hex The hex string to create the public key from.
         * @throws If the hex string has the wrong length for a public key.
         * @throws If the public key is the point at infinity.
         * @returns A new PublicKey.
         */
        static fromHex(hex) {
          return new _PublicKey(Point2.fromHex(hex, true));
        }
        /**
         * Returns the public key as a hex string.
         * @returns A 64 character hex string.
         */
        toHex() {
          return this.key.toHex();
        }
        /**
         * Returns the preferred JSON encoding of the public key.
         * @returns A 64 character hex string.
         */
        toJSON() {
          return this.toHex();
        }
      };
      PublicKey.LENGTH = 32;
      PublicKeyWithChainCode3 = class _PublicKeyWithChainCode {
        /**
         * @param public_key The public key.
         * @param chain_code A hash of the derivation path.
         */
        constructor(public_key, chain_code) {
          this.public_key = public_key;
          this.chain_code = chain_code;
        }
        /**
         * Return the master public key used in the production mainnet
         */
        static forMainnetKey(key_id) {
          const chain_key = ChainCode.fromHex("0000000000000000000000000000000000000000000000000000000000000000");
          if (key_id === "key_1") {
            const public_key = PublicKey.fromHex("476374d9df3a8af28d3164dc2422cff894482eadd1295290b6d9ad92b2eeaa5c");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else if (key_id === "test_key_1") {
            const public_key = PublicKey.fromHex("6c0824beb37621bcca6eecc237ed1bc4e64c9c59dcb85344aa7f9cc8278ee31f");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else {
            throw new Error("Unknown master public key id");
          }
        }
        /**
         * Return the master public key used by PocketIC for testing
         */
        static forPocketIcKey(key_id) {
          const chain_key = ChainCode.fromHex("0000000000000000000000000000000000000000000000000000000000000000");
          if (key_id === "key_1") {
            const public_key = PublicKey.fromHex("db415b8eb85bd5127b0984723e0448054042cf40e7a9c262ed0cc87ecea98349");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else if (key_id === "test_key_1") {
            const public_key = PublicKey.fromHex("6ed9121ecf701b9e301fce17d8a65214888984e8211225691b089d6b219ec144");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else if (key_id === "dfx_test_key") {
            const public_key = PublicKey.fromHex("7124afcb1be5927cac0397a7447b9c3cda2a4099af62d9bc0a2c2fe42d33efe1");
            return new _PublicKeyWithChainCode(public_key, chain_key);
          } else {
            throw new Error("Unknown master public key id");
          }
        }
        /**
         * Creates a new PublicKeyWithChainCode from two hex strings.
         * @param public_key_hex The public key in hex format.
         * @param chain_code_hex The chain code in hex format.
         * @returns A new PublicKeyWithChainCode.
         */
        static fromHex(public_key_hex, chain_code_hex) {
          const public_key = PublicKey.fromHex(public_key_hex);
          const chain_key = ChainCode.fromHex(chain_code_hex);
          return new _PublicKeyWithChainCode(public_key, chain_key);
        }
        /**
         * Creates a new PublicKeyWithChainCode from two strings.
         * @param public_key_string The public key in any format supported by PublicKey.fromString.
         * @param chain_code_string The chain code in any format supported by ChainCode.fromString.
         * @returns A new PublicKeyWithChainCode.
         */
        static fromString(public_key_string, chain_code_string) {
          const public_key = PublicKey.fromString(public_key_string);
          const chain_code = ChainCode.fromString(chain_code_string);
          return new _PublicKeyWithChainCode(public_key, chain_code);
        }
        /**
         * Applies the given derivation path to obtain a new public key and chain code.
         *
         * Corresponds to rust: [`ic_ed25519::PublicKey::derive_public_key_with_chain_code()`](https://github.com/dfinity/ic/blob/e915efecc8af90993ccfc499721ebe826aadba60/packages/ic-ed25519/src/lib.rs#L774C1-L793C6)
         */
        deriveSubkeyWithChainCode(derivationPath) {
          const [pt, _sum, chainCode] = derivationPath.deriveOffset(this.public_key.key, this.chain_code);
          return new _PublicKeyWithChainCode(new PublicKey(pt), chainCode);
        }
      };
      DerivationPath2 = class _DerivationPath {
        constructor(path) {
          this.path = path;
        }
        /**
         * Creates a new DerivationPath from a canister identifier plus other path components
         * @param canisterId the id of the canister
         * @param path other path components
         */
        static withCanisterPrefix(canisterId, path) {
          const canisterIdBytes = canisterId.toUint8Array();
          const newPath = [canisterIdBytes, ...path];
          return new _DerivationPath(newPath);
        }
        /**
         * Creates a new DerivationPath from / separated candid blobs.
         * @param blob The / separated blobs to create the derivation path from.
         * @returns A new DerivationPath.
         */
        static fromBlob(blob) {
          if (blob === void 0 || blob === null) {
            return new _DerivationPath([]);
          }
          return new _DerivationPath(blob.split("/").map((p) => blobDecode(p)));
        }
        /**
         * @returns A string representation of the derivation path: Candid blob encoded with a '/' between each path component.  Or `null` for a derivation path with no components.
         */
        toBlob() {
          if (this.path.length === 0) {
            return null;
          }
          return this.path.map((p) => blobEncode(p)).join("/");
        }
        /**
         * Returns the preferred JSON encoding of the derivation path.
         * @returns A blob-encoded string with '/' separating components, or `null` if the path has no components.
         */
        toJSON() {
          return this.toBlob();
        }
        /**
         * A typescript translation of [ic_ed25519::DerivationPath::derive_offset](https://github.com/dfinity/ic/blob/e915efecc8af90993ccfc499721ebe826aadba60/packages/ic-ed25519/src/lib.rs#L849).
         * @param pt The public key to derive the offset from.
         * @param chainCode The chain code to derive the offset from.
         * @returns A tuple containing the derived public key, the offset, and the chain code.
         */
        deriveOffset(pt, chainCode) {
          return this.path.reduce(deriveOneOffset, [pt, 0n, chainCode]);
        }
      };
    }
  });

  // node_modules/@dfinity/ic-pub-key/dist/schnorr/index.js
  var schnorr_exports = {};
  __export(schnorr_exports, {
    bip340secp256k1: () => bip340secp256k1_exports,
    ed25519: () => ed25519_exports
  });
  var init_schnorr = __esm({
    "node_modules/@dfinity/ic-pub-key/dist/schnorr/index.js"() {
      init_bip340secp256k1();
      init_ed255192();
    }
  });

  // node_modules/@dfinity/ic-pub-key/dist/index.js
  var init_dist = __esm({
    "node_modules/@dfinity/ic-pub-key/dist/index.js"() {
      init_chain_code();
      init_ecdsa();
      init_schnorr();
    }
  });

  // entry2.js
  var require_entry2 = __commonJS({
    "entry2.js"() {
      init_dist();
      window.deriveDepositAddress = function(pubkeyHex, chaincodeHex, principalText, subaccountHex) {
        const { ed25519 } = schnorr_exports;
        function base32Decode(text) {
          const ALPHABET = "abcdefghijklmnopqrstuvwxyz234567";
          let width = 0, acc = 0, bytes = [];
          for (let i = 0; i < text.length; i++) {
            let b = ALPHABET.indexOf(text.charAt(i));
            if (b === -1) throw Error("Invalid base32 character: " + text.charAt(i));
            acc = (acc << 5) + b;
            width += 5;
            if (width >= 8) {
              bytes.push(acc >> width - 8);
              acc &= (1 << width - 8) - 1;
              width -= 8;
            }
          }
          if (acc > 0) throw Error("Invalid principal: non-zero padding");
          return bytes;
        }
        function principalToBytes(text) {
          let ungroup = text.replace(/-/g, "").toLowerCase();
          let raw = base32Decode(ungroup);
          if (raw.length < 4) throw Error("Invalid principal: too short");
          return new Uint8Array(raw.slice(4));
        }
        function hexToBytes3(hex) {
          const bytes = new Uint8Array(hex.length / 2);
          for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(hex.substr(i * 2, 2), 16);
          return bytes;
        }
        function base58Encode(bytes) {
          const ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
          let digits = [0];
          for (let b of bytes) {
            let carry = b;
            for (let j = 0; j < digits.length; j++) {
              carry += digits[j] << 8;
              digits[j] = carry % 58;
              carry = carry / 58 | 0;
            }
            while (carry > 0) {
              digits.push(carry % 58);
              carry = carry / 58 | 0;
            }
          }
          let str = "";
          for (let i = 0; i < bytes.length && bytes[i] === 0; i++) str += "1";
          for (let i = digits.length - 1; i >= 0; i--) str += ALPHABET[digits[i]];
          return str;
        }
        const principalBytes = principalToBytes(principalText);
        let subaccountBytes = new Uint8Array(32);
        if (subaccountHex && subaccountHex.length > 0) {
          const hex = subaccountHex.replace(/^0x/, "");
          if (hex.length !== 64) throw Error("Subaccount must be 32 bytes (64 hex chars)");
          subaccountBytes = hexToBytes3(hex);
        }
        const path = new ed25519.DerivationPath([
          new Uint8Array([1]),
          principalBytes,
          subaccountBytes
        ]);
        const pk = ed25519.PublicKeyWithChainCode.fromHex(pubkeyHex, chaincodeHex);
        const derived = pk.deriveSubkeyWithChainCode(path);
        const derivedHex = derived.public_key.toHex();
        return base58Encode(hexToBytes3(derivedHex));
      };
    }
  });
  require_entry2();
})();
/*! Bundled license information:

@noble/hashes/esm/utils.js:
  (*! noble-hashes - MIT License (c) 2022 Paul Miller (paulmillr.com) *)

@noble/secp256k1/index.js:
  (*! noble-secp256k1 - MIT License (c) 2019 Paul Miller (paulmillr.com) *)

@noble/ed25519/index.js:
  (*! noble-ed25519 - MIT License (c) 2019 Paul Miller (paulmillr.com) *)
*/
