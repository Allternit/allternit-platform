// Same name and description as the runtime builtin (runtime/tools/builtins/request-signature.ts).
import REQUEST_SIGNATURE_TXT from '@/runtime/tools/builtins/request-signature.txt'

export const REQUEST_SIGNATURE_TOOL_NAME = 'request_signature'

export const DESCRIPTION = 'Ask the user to sign a PDF; they sign on Desktop or phone'

export const PROMPT: string = `${REQUEST_SIGNATURE_TXT}
The terminal cannot sign: the user opens the link on Desktop or their phone. There is no key or command here that signs or declines.`
