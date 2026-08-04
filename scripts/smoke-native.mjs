import { spawn } from 'node:child_process'
import { resolve } from 'node:path'

const PROTOCOL_VERSION = 5
const executable = resolve(process.argv[2] ?? 'native/out/windows-x86_64/bin/backend.exe')
const child = spawn(executable, [], {
  env: { ...process.env, MYWALLPAPER_PROTOCOL: 'process-v2' },
  stdio: ['pipe', 'pipe', 'inherit'],
})

let buffer = Buffer.alloc(0)
let ready = false
let frame = false
const timeout = setTimeout(() => finish(new Error('native audio smoke test timed out')), 8_000)

child.stdout.on('data', (chunk) => {
  buffer = Buffer.concat([buffer, chunk])
  while (buffer.length >= 4) {
    const length = buffer.readUInt32LE(0)
    if (length === 0 || length > 1024 * 1024) return finish(new Error('invalid native frame length'))
    if (buffer.length < length + 4) return
    const message = JSON.parse(buffer.subarray(4, length + 4).toString('utf8'))
    buffer = buffer.subarray(length + 4)
    if (message.type === 'ready') ready = true
    if (message.type === 'message' && message.payload?.kind === 'audio.frame') {
      frame = Array.isArray(message.payload.bands) && message.payload.bands.length === 64
      if (ready && frame) finish()
    }
    if (message.type === 'message' && message.payload?.kind === 'audio.error') {
      finish(new Error(message.payload.message))
    }
    if (message.type === 'error') finish(new Error(`${message.code}: ${message.message}`))
  }
})
child.on('error', finish)
child.on('exit', (code) => { if (!frame) finish(new Error(`native companion exited with ${code}`)) })

write({ type: 'init', v: PROTOCOL_VERSION, layerSettings: {}, deviceSettings: { refreshRate: '30fps' } })

function write(value) {
  const payload = Buffer.from(JSON.stringify(value))
  const prefix = Buffer.alloc(4)
  prefix.writeUInt32LE(payload.length)
  child.stdin.write(Buffer.concat([prefix, payload]))
}

let finished = false
function finish(error) {
  if (finished) return
  finished = true
  clearTimeout(timeout)
  try { write({ type: 'shutdown', v: PROTOCOL_VERSION }) } catch {}
  child.stdin.end()
  if (error) {
    console.error(error.message)
    process.exitCode = 1
  } else {
    console.log('Native WASAPI loopback smoke test passed.')
  }
}
