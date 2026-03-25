import { useState, useEffect, useCallback } from 'react';
import { EscrowService } from '../services/escrow.service';
import { Escrow, CreateEscrowParams } from '../types/escrow.types';

export const useEscrow = (contractId: string, rpcUrl: string) => {
  const [escrowService] = useState(() => new EscrowService(contractId, rpcUrl));
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchEscrow = useCallback(async (id: number) => {
    setLoading(true);
    setError(null);
    try {
      const data = await escrowService.getEscrow(id);
      return data;
    } catch (err: any) {
      setError(err.message || 'Failed to fetch escrow');
      return null;
    } finally {
      setLoading(false);
    }
  }, [escrowService]);

  const fetchEscrowCount = useCallback(async () => {
    setLoading(true);
    try {
      return await escrowService.getEscrowCount();
    } catch (err: any) {
      setError(err.message || 'Failed to fetch escrow count');
      return 0;
    } finally {
      setLoading(false);
    }
  }, [escrowService]);

  // Hook could also expose transaction submission functions
  // for create, release, dispute, etc.

  return {
    loading,
    error,
    fetchEscrow,
    fetchEscrowCount,
  };
};
