/**
 * Cloud client for Link board relay registration: interface only (Phase 1,
 * lane 0). hardware/link/README.md section 4.5. Lane D adds httpLinkBoardCloud.
 */

export interface LinkBoardCloud {
  register(boardId: string, name: string): Promise<{ relayToken: string; relayHost: string }>;
  revoke(boardId: string): Promise<void>;
  list(): Promise<Array<{ boardId: string; revokedAt: string | null }>>;
}
