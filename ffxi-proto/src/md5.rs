use md5::{Digest, Md5};

pub fn md5_into(data: &[u8], out: &mut [u8; 16]) {
    let digest = Md5::digest(data);
    out.copy_from_slice(&digest);
}

pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    md5_into(data, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc1321_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (b"", "d41d8cd98f00b204e9800998ecf8427e"),
            (b"a", "0cc175b9c0f1b6a831c399e269772661"),
            (b"abc", "900150983cd24fb0d6963f7d28e17f72"),
            (b"message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                b"abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, expected_hex) in cases {
            let got = md5(input);
            let got_hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(&got_hex, expected_hex, "input {input:?}");
        }
    }
}
