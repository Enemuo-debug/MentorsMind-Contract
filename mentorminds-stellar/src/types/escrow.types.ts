export enum EscrowStatus {
  Active = "Active",
  Released = "Released",
  Disputed = "Disputed",
  Refunded = "Refunded",
  Resolved = "Resolved",
}

export interface Escrow {
  id: number;
  mentor: string;
  learner: string;
  amount: bigint;
  sessionId: string;
  status: EscrowStatus;
  createdAt: number;
  tokenAddress: string;
  platformFee: bigint;
  netAmount: bigint;
  sessionEndTime: number;
  autoReleaseDelay: number;
  disputeReason: string;
  resolvedAt: number;
}

export interface CreateEscrowParams {
  mentor: string;
  learner: string;
  amount: bigint | number;
  sessionId: string;
  tokenAddress: string;
  sessionEndTime: number;
}

export interface EscrowEvent {
  type: 'created' | 'released' | 'rel_part' | 'auto_rel' | 'disp_opnd' | 'disp_res' | 'refunded' | 'review';
  escrowId: number;
  data: any;
}
