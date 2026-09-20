//! Trae CN "tc" 登录态加密格式的读写实现（切换流程用其加密写入，账号识别用其解密读取）。
//!
//! 背景：新版 Trae CN（实测 2.3.85576）把 storage.json 中的 `iCubeAuthInfo://icube.cloudide`
//! 从明文 JSON 改为自定义加密格式，导致旧版"写明文登录态"的切换机制失效（表现为切换后要重新登录）。
//! 格式逆向自开源项目 dsh-trae-api 的 trae-decrypt.js，并经本机真实密文验证：
//!
//!   密文 = base64( [6B 头 74 63 05 10 00 00] [32B 随机数] [AES-128-CBC/PKCS7 密文] )
//!   明文 = [64B SHA-512(正文)] [正文 JSON]
//!   派生 = SHA-512( SHA-512(随机数) + (SALT_A ^ SALT_B) )，前 16B 作 key，后 16B 作 IV
//!
//! 注意：该格式的密钥完全由密文内嵌随机数派生（盐为硬编码常量），仅用于兼容 Trae 的存储
//! 格式，不提供真实机密性；也不要求 Local State/os_crypt 存在，故切换流程可安全删除它们。

use aes::Aes128;
use anyhow::{anyhow, Result};
use base64::Engine;
use cbc::cipher::{BlockEncryptMut, KeyIvInit};
use sha2::{Digest, Sha512};

/// tc 格式固定头（"tc" + 版本 05 + 子类型 10 00 00）
const TC_HEADER: [u8; 6] = [0x74, 0x63, 0x05, 0x10, 0x00, 0x00];

/// 盐数组取自 Trae 前端代码（A ^ B 后参与派生；AES_PRIVATE 变体用 C ^ D，本项目不涉及）
const SALT_A: [u8; 64] = [
    82, 9, 106, 213, 48, 54, 165, 56, 191, 64, 163, 158, 129, 243, 215, 251, 124, 227, 57, 130,
    155, 47, 255, 135, 52, 142, 67, 68, 196, 222, 233, 203, 84, 123, 148, 50, 166, 194, 35, 61,
    238, 76, 149, 11, 66, 250, 195, 78, 8, 46, 161, 102, 40, 217, 36, 178, 118, 91, 162, 73, 109,
    139, 209, 37,
];

const SALT_B: [u8; 64] = [
    31, 221, 168, 51, 136, 7, 199, 49, 177, 18, 16, 89, 39, 128, 236, 95, 96, 81, 127, 169, 25,
    181, 74, 13, 45, 229, 122, 159, 147, 201, 156, 239, 160, 224, 59, 77, 174, 42, 245, 176, 200,
    235, 187, 60, 131, 83, 153, 97, 23, 43, 4, 126, 186, 119, 214, 38, 225, 105, 20, 99, 85, 33,
    12, 125,
];

/// 派生 AES-128 key 与 IV：SHA-512(SHA-512(random) + salt)，前 16B 为 key、后 16B 为 IV
fn derive_key_iv(random: &[u8; 32]) -> ([u8; 16], [u8; 16]) {
    let salt: Vec<u8> = SALT_A
        .iter()
        .zip(SALT_B.iter())
        .map(|(a, b)| a ^ b)
        .collect();

    let h1 = Sha512::digest(random);
    let mut h2 = Sha512::new();
    h2.update(h1);
    h2.update(&salt);
    let h2 = h2.finalize();

    let mut key = [0u8; 16];
    let mut iv = [0u8; 16];
    key.copy_from_slice(&h2[0..16]);
    iv.copy_from_slice(&h2[16..32]);
    (key, iv)
}

/// 将明文 JSON 按 tc 格式加密为 base64 字符串（写入 storage.json 用）
pub fn encrypt_storage_value(plaintext: &str) -> Result<String> {
    // 随机数每次重新生成：它决定派生密钥，重复使用会削弱本就有限的混淆强度
    let mut random = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut random);

    let (key, iv) = derive_key_iv(&random);

    // 明文前附加 SHA-512 摘要，Trae 解密时会校验，缺失会被判定为损坏
    let body_hash = Sha512::digest(plaintext.as_bytes());
    let mut payload = Vec::with_capacity(64 + plaintext.len());
    payload.extend_from_slice(&body_hash);
    payload.extend_from_slice(plaintext.as_bytes());

    let ciphertext = cbc::Encryptor::<Aes128>::new((&key).into(), (&iv).into())
        .encrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(&payload);

    let mut out = Vec::with_capacity(6 + 32 + ciphertext.len());
    out.extend_from_slice(&TC_HEADER);
    out.extend_from_slice(&random);
    out.extend_from_slice(&ciphertext);

    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

/// 解密 tc 格式
///
/// 除了测试与调试，读取侧也依赖它：「读取本地 Trae IDE 账号」（`account_manager`）与 TraeWork 的
/// 账号识别（`traework::uid`）都要把客户端写下的密文还原成 JSON。写入侧只需加密，故本函数曾长期
/// 标注 `#[allow(dead_code)]`，勿再移除消费方而不恢复该标注。
pub fn decrypt_storage_value(b64: &str) -> Result<String> {
    use cbc::cipher::BlockDecryptMut;
    let buf = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| anyhow!("base64 解码失败: {}", e))?;
    if buf.len() < 6 + 32 + 16 || buf[0..6] != TC_HEADER {
        return Err(anyhow!("不是 tc 格式密文"));
    }
    let random: &[u8; 32] = buf[6..38].try_into().unwrap();
    let (key, iv) = derive_key_iv(random);
    let plain = cbc::Decryptor::<Aes128>::new((&key).into(), (&iv).into())
        .decrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(&buf[38..])
        .map_err(|e| anyhow!("AES 解密失败: {}", e))?;
    if plain.len() < 64 || Sha512::digest(&plain[64..])[..] != plain[..64] {
        return Err(anyhow!("SHA-512 校验失败"));
    }
    Ok(String::from_utf8_lossy(&plain[64..]).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 自加密自解密回环 + 输出一份样本供 dsh-trae-api 交叉验证
    #[test]
    fn roundtrip() {
        let plain = r#"{"token":"TEST_TOKEN","userId":"123","host":"https://api.trae.com.cn"}"#;
        let enc = encrypt_storage_value(plain).unwrap();
        let dec = decrypt_storage_value(&enc).unwrap();
        assert_eq!(dec, plain);
        // 交叉验证样本：用 dsh-trae-api 的 decryptStorageValue 解此串应得到同一明文
        println!("TC_SAMPLE={}", enc);
    }
}
