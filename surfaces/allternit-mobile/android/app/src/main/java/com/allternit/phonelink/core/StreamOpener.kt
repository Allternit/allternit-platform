package com.allternit.phonelink.core

import java.net.Socket

/** Opens one raw shell-server stream (a [ShellServerProtocol] command, already validated). */
fun interface StreamOpener {
    fun open(command: String): Socket
}
