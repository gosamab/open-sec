// Minimal stub so the focus file imports cleanly when read by tools.
export const db = {
  async query(_sql: string): Promise<unknown[]> {
    return [];
  },
};
