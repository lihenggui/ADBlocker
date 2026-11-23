// Copyright (C) 2026 Mercury Li
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 3 as
// published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Use rustls to send real TLS flights, with a dummy HPKE sealer because
//! these tests only cover servers that cannot open the inner hello.

use rustls::Error;
use rustls::client::{EchConfig, EchGreaseConfig, EchMode};
use rustls::crypto::hpke::{
    EncapsulatedSecret, Hpke, HpkeOpener, HpkePrivateKey, HpkePublicKey, HpkeSealer, HpkeSuite,
};
use rustls::internal::msgs::enums::{HpkeAead, HpkeKdf, HpkeKem};
use rustls::internal::msgs::handshake::HpkeSymmetricCipherSuite;

#[derive(Debug)]
struct TestHpke;

impl Hpke for TestHpke {
    fn setup_sealer(
        &self,
        _: &[u8],
        _: &HpkePublicKey,
    ) -> Result<(EncapsulatedSecret, Box<dyn HpkeSealer>), Error> {
        Ok((EncapsulatedSecret(vec![0x42; 32]), Box::new(TestHpke)))
    }

    fn suite(&self) -> HpkeSuite {
        HpkeSuite {
            kem: HpkeKem::DHKEM_X25519_HKDF_SHA256,
            sym: HpkeSymmetricCipherSuite {
                kdf_id: HpkeKdf::HKDF_SHA256,
                aead_id: HpkeAead::AES_128_GCM,
            },
        }
    }

    fn seal(
        &self,
        _: &[u8],
        _: &[u8],
        _: &[u8],
        _: &HpkePublicKey,
    ) -> Result<(EncapsulatedSecret, Vec<u8>), Error> {
        unreachable!()
    }

    fn open(
        &self,
        _: &EncapsulatedSecret,
        _: &[u8],
        _: &[u8],
        _: &[u8],
        _: &HpkePrivateKey,
    ) -> Result<Vec<u8>, Error> {
        unreachable!()
    }

    fn setup_opener(
        &self,
        _: &EncapsulatedSecret,
        _: &[u8],
        _: &HpkePrivateKey,
    ) -> Result<Box<dyn HpkeOpener>, Error> {
        unreachable!()
    }

    fn generate_key_pair(&self) -> Result<(HpkePublicKey, HpkePrivateKey), Error> {
        unreachable!()
    }
}

impl HpkeSealer for TestHpke {
    fn seal(&mut self, _: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, Error> {
        Ok(vec![0xa5; plaintext.len() + 16])
    }
}

pub(crate) fn grease() -> EchMode {
    EchGreaseConfig::new(&TestHpke, HpkePublicKey(vec![0x42; 32])).into()
}

pub(crate) fn offer(public_name: &str) -> EchMode {
    let mut contents = vec![7, 0, 0x20, 0, 32]; // config ID, X25519 KEM, key length
    contents.extend_from_slice(&[0x42; 32]);
    contents.extend_from_slice(&[0, 4, 0, 1, 0, 1]); // HKDF-SHA256, AES-128-GCM
    contents.extend_from_slice(&[0, public_name.len() as u8]); // max name length, public name
    contents.extend_from_slice(public_name.as_bytes());
    contents.extend_from_slice(&[0, 0]); // no extensions
    let mut configs = Vec::new();
    configs.extend_from_slice(&((contents.len() + 4) as u16).to_be_bytes());
    configs.extend_from_slice(&[0xfe, 0x0d]);
    configs.extend_from_slice(&(contents.len() as u16).to_be_bytes());
    configs.extend_from_slice(&contents);
    EchConfig::new(configs.into(), &[&TestHpke]).unwrap().into()
}
