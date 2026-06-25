const { Keypair, VersionedTransaction } = require('@solana/web3.js');
const fs = require('fs');

const keypairPath = process.argv[2];
const base64Tx = process.argv[3];

if (!keypairPath || !base64Tx) {
  console.error('Usage: node sign.js <keypair_path> <base64_tx>');
  process.exit(1);
}

const secretKey = Uint8Array.from(JSON.parse(fs.readFileSync(keypairPath, 'utf-8')));
const keypair = Keypair.fromSecretKey(secretKey);
const txBytes = Buffer.from(base64Tx, 'base64');
const tx = VersionedTransaction.deserialize(txBytes);
tx.sign([keypair]);
console.log(Buffer.from(tx.serialize()).toString('base64'));
