/** Where to connect for the socket `allternit-factory pane tty ensure`
 *  reports. On Windows the pane engine listens on the named pipe
 *  `\\.\pipe\<that path>` (the path itself is a marker file), the same mapping
 *  as allternit-api's terminal_routes.rs `pipe_name`; elsewhere it is a Unix
 *  socket path, used as is. */
export function paneEndpointFor(socket: string, platform: NodeJS.Platform = process.platform): string {
  if (platform !== "win32" || socket.startsWith("\\\\.\\pipe\\")) return socket
  return "\\\\.\\pipe\\" + socket
}
