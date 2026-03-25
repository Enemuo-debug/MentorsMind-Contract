import { 
  Keypair, 
  rpc, 
  TransactionBuilder, 
  Networks, 
  Contract, 
  nativeToScVal 
} from '@stellar/stellar-sdk';

export class AdminEscrowService {
  private contract: Contract;
  private server: rpc.Server;
  private adminKeypair: Keypair;

  constructor(contractId: string, rpcUrl: string, adminSecret: string) {
    this.contract = new Contract(contractId);
    this.server = new rpc.Server(rpcUrl);
    this.adminKeypair = Keypair.fromSecret(adminSecret);
  }

  async resolveDispute(escrowId: number, releaseToMentor: boolean): Promise<string> {
    const account = await this.server.getLatestLedger();
    const sourceAccount = await this.server.getAccount(this.adminKeypair.publicKey());

    const operation = this.contract.call(
      'resolve_dispute',
      nativeToScVal(escrowId, { type: 'u64' }),
      nativeToScVal(releaseToMentor, { type: 'bool' })
    );

    const transaction = new TransactionBuilder(sourceAccount, {
      fee: '1000',
      networkPassphrase: Networks.TESTNET, // Or configured network
    })
    .addOperation(operation)
    .build();

    transaction.sign(this.adminKeypair);
    
    const sendResponse = await this.server.sendTransaction(transaction);
    if (sendResponse.status !== 'PENDING') {
      throw new Error(`Failed to send transaction: ${sendResponse.status}`);
    }

    return sendResponse.hash;
  }

  async refund(escrowId: number): Promise<string> {
    // Similar implementation for refund
    const sourceAccount = await this.server.getAccount(this.adminKeypair.publicKey());
    const operation = this.contract.call('refund', nativeToScVal(escrowId, { type: 'u64' }));
    const transaction = new TransactionBuilder(sourceAccount, { fee: '1000', networkPassphrase: Networks.TESTNET })
      .addOperation(operation)
      .build();
    transaction.sign(this.adminKeypair);
    const res = await this.server.sendTransaction(transaction);
    return res.hash;
  }
}
