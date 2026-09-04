'use strict';
// Primary-source conformance check: Node's Ed25519 must reproduce the test
// vectors of RFC 8032 §7.1 (EdDSA: Ed25519 and Ed448, January 2017),
// https://www.rfc-editor.org/rfc/rfc8032 — fetched and transcribed verbatim
// during the GH-609 spike, 2026-09-04.
//
// This pins the crypto PRIMITIVE to the RFC, independently of Node's own
// docs, before any edda signing logic is layered on top of it.

const crypto = require('node:crypto');

/** RFC 8032 §7.1, TEST 1 (empty message). */
const TEST_1 = {
  secretKey:
    '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60',
  publicKey:
    'd75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a',
  message: '',
  signature:
    'e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155' +
    '5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b',
};

/** RFC 8032 §7.1, TEST 3 (two-byte message 0xaf82). */
const TEST_3 = {
  secretKey:
    'c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7',
  publicKey:
    'fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025',
  message: 'af82',
  signature:
    '6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac' +
    '18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a',
};

/**
 * @param {{secretKey: string, publicKey: string, message: string, signature: string}} tv
 * @returns {{pubMatches: boolean, sigMatches: boolean}}
 */
function checkVector(tv) {
  const sk = Buffer.from(tv.secretKey, 'hex');
  // PKCS8 Ed25519 DER: 302e020100300506032b657004220420 || 32-byte seed
  const priv = crypto.createPrivateKey({
    key: Buffer.concat([
      Buffer.from('302e020100300506032b657004220420', 'hex'),
      sk,
    ]),
    format: 'der',
    type: 'pkcs8',
  });
  const derivedPub = crypto
    .createPublicKey(priv)
    .export({ type: 'spki', format: 'der' })
    .subarray(-32)
    .toString('hex');
  const pubMatches = derivedPub === tv.publicKey;
  const sig = crypto
    .sign(null, Buffer.from(tv.message, 'hex'), priv)
    .toString('hex');
  const sigMatches = sig === tv.signature;
  // and the RFC signature verifies under the RFC public key
  const pubDer = Buffer.concat([
    Buffer.from('302a300506032b6570032100', 'hex'),
    Buffer.from(tv.publicKey, 'hex'),
  ]);
  const verified = crypto.verify(
    null,
    Buffer.from(tv.message, 'hex'),
    crypto.createPublicKey({ key: pubDer, format: 'der', type: 'spki' }),
    Buffer.from(tv.signature, 'hex'),
  );
  return { pubMatches, sigMatches, verified };
}

module.exports = { TEST_1, TEST_3, checkVector };
