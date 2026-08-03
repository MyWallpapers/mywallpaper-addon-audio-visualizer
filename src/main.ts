import './styles.css'
import type { CanvasAddonMountContext, JsonValue, NativeConnection } from '../generated/mywallpaper-runtime'

type VisualStyle = 'bars' | 'wave' | 'mirrored'

interface Settings {
  style: VisualStyle
  barCount: number
  sensitivity: number
  smoothing: number
  primaryColor: string
  secondaryColor: string
  backgroundColor: string
  backgroundOpacity: number
  glow: number
  cornerRadius: number
}

interface AudioFrame {
  kind: 'audio.frame'
  capturedAtUnixMs: number
  level: number
  peak: number
  bands: number[]
}

const defaults: Settings = {
  style: 'bars', barCount: 48, sensitivity: 1.6, smoothing: 0.72,
  primaryColor: '#7c5cff', secondaryColor: '#39d9ff', backgroundColor: '#070912',
  backgroundOpacity: 0.35, glow: 18, cornerRadius: 20,
}

export function mount({ layer, runtime, bus }: CanvasAddonMountContext): () => void {
  const root = layer.root
  root.classList.add('audio-visualizer-root')
  root.innerHTML = '<canvas aria-label="Live Windows audio visualization"></canvas><p class="status">Connecting to Windows audio…</p>'
  const canvas = required<HTMLCanvasElement>('canvas')
  const status = required<HTMLParagraphElement>('.status')
  const context = requireCanvasContext(canvas)

  let settings = readSettings(layer.settings.get())
  let connection: NativeConnection | null = null
  let targetBands = new Float32Array(96)
  let visibleBands = new Float32Array(96)
  let level = 0
  let peak = 0
  let disposed = false
  let animationFrame = 0

  const stopSettings = layer.settings.subscribe((value) => { settings = readSettings(value) })
  const resize = new ResizeObserver(resizeCanvas)
  resize.observe(root)
  resizeCanvas()
  animationFrame = requestAnimationFrame(draw)
  void connect()

  async function connect(): Promise<void> {
    if (!layer.native.companion.available) {
      showStatus('Windows audio capture is unavailable. Enable this add-on’s native capability in MyWallpaper Desktop.', 'error')
      return
    }
    try {
      const next = await layer.native.companion.connect()
      if (disposed) { next.close(); return }
      connection = next
      next.onStateChange((state) => {
        if (state === 'open') showStatus('Waiting for audio…', 'neutral')
        else if (state === 'reconnecting') showStatus('Reconnecting to Windows audio…', 'warning')
        else showStatus('Audio capture stopped. Open Settings → Add-ons for the native runtime cause.', 'error')
      })
      next.onMessage(receive)
    } catch (error) {
      showStatus(`Audio capture could not start: ${error instanceof Error ? error.message : String(error)}`, 'error')
    }
  }

  function receive(payload: JsonValue): void {
    if (!isRecord(payload)) return
    if (payload['kind'] === 'audio.error' && typeof payload['message'] === 'string') {
      showStatus(`Audio capture failed: ${payload['message']}`, 'error')
      return
    }
    if (!isAudioFrame(payload)) return
    const frame = payload as unknown as AudioFrame
    targetBands.fill(0)
    for (let index = 0; index < Math.min(frame.bands.length, targetBands.length); index += 1) {
      targetBands[index] = clamp(frame.bands[index] ?? 0, 0, 1)
    }
    level = clamp(frame.level, 0, 1)
    peak = clamp(frame.peak, 0, 1)
    status.hidden = level > 0.002
    if (runtime.instance.canonical) bus.emit('mywallpaper.audio/v1/frame', payload)
  }

  function draw(): void {
    const width = canvas.width
    const height = canvas.height
    context.clearRect(0, 0, width, height)
    context.save()
    roundedRect(context, 0, 0, width, height, settings.cornerRadius * devicePixelRatio)
    context.clip()
    context.globalAlpha = settings.backgroundOpacity
    context.fillStyle = settings.backgroundColor
    context.fillRect(0, 0, width, height)
    context.globalAlpha = 1

    const count = Math.round(settings.barCount)
    const attack = 1 - settings.smoothing * 0.65
    const release = 1 - settings.smoothing * 0.18
    for (let index = 0; index < count; index += 1) {
      const sourceIndex = Math.floor(index / Math.max(1, count - 1) * (targetBands.length - 1))
      const target = clamp((targetBands[sourceIndex] ?? 0) * settings.sensitivity, 0, 1)
      const current = visibleBands[index] ?? 0
      visibleBands[index] = current + (target - current) * (target > current ? attack : release)
    }

    context.shadowBlur = settings.glow * devicePixelRatio
    context.shadowColor = settings.secondaryColor
    const gradient = context.createLinearGradient(0, height, width, 0)
    gradient.addColorStop(0, settings.primaryColor)
    gradient.addColorStop(1, settings.secondaryColor)
    context.fillStyle = gradient
    context.strokeStyle = gradient
    if (settings.style === 'wave') drawWave(context, width, height, visibleBands, count)
    else drawBars(context, width, height, visibleBands, count, settings.style === 'mirrored')
    context.restore()
    animationFrame = requestAnimationFrame(draw)
  }

  function resizeCanvas(): void {
    const rect = root.getBoundingClientRect()
    const scale = Math.min(devicePixelRatio || 1, 2)
    canvas.width = Math.max(1, Math.round(rect.width * scale))
    canvas.height = Math.max(1, Math.round(rect.height * scale))
  }

  function showStatus(message: string, tone: 'neutral' | 'warning' | 'error'): void {
    status.hidden = false
    status.dataset['tone'] = tone
    status.textContent = message
  }

  function required<T extends Element>(selector: string): T {
    const element = root.querySelector<T>(selector)
    if (!element) throw new Error(`Audio Visualizer UI is missing ${selector}`)
    return element
  }

  return () => {
    disposed = true
    cancelAnimationFrame(animationFrame)
    resize.disconnect()
    stopSettings()
    connection?.close()
    root.classList.remove('audio-visualizer-root')
    root.replaceChildren()
  }
}

function drawBars(context: CanvasRenderingContext2D, width: number, height: number, values: Float32Array, count: number, mirrored: boolean): void {
  const gap = Math.max(2, width / count * 0.18)
  const barWidth = Math.max(1, (width - gap * (count + 1)) / count)
  for (let index = 0; index < count; index += 1) {
    const value = values[index] ?? 0
    const barHeight = Math.max(2, value * height * (mirrored ? 0.46 : 0.9))
    const x = gap + index * (barWidth + gap)
    const y = mirrored ? height / 2 - barHeight : height - barHeight
    context.beginPath()
    context.roundRect(x, y, barWidth, mirrored ? barHeight * 2 : barHeight, Math.min(barWidth / 2, 8))
    context.fill()
  }
}

function drawWave(context: CanvasRenderingContext2D, width: number, height: number, values: Float32Array, count: number): void {
  context.lineWidth = Math.max(2, height * 0.018)
  context.lineJoin = 'round'
  context.lineCap = 'round'
  context.beginPath()
  for (let index = 0; index < count; index += 1) {
    const x = index / Math.max(1, count - 1) * width
    const direction = index % 2 === 0 ? -1 : 1
    const y = height / 2 + direction * (values[index] ?? 0) * height * 0.42
    if (index === 0) context.moveTo(x, y); else context.lineTo(x, y)
  }
  context.stroke()
}

function roundedRect(context: CanvasRenderingContext2D, x: number, y: number, width: number, height: number, radius: number): void {
  context.beginPath()
  context.roundRect(x, y, width, height, Math.min(radius, width / 2, height / 2))
}

function requireCanvasContext(canvas: HTMLCanvasElement): CanvasRenderingContext2D {
  const context = canvas.getContext('2d', { alpha: true })
  if (!context) throw new Error('Audio Visualizer requires Canvas 2D support')
  return context
}

function readSettings(value: Record<string, JsonValue>): Settings {
  const style = value['style'] === 'wave' || value['style'] === 'mirrored' ? value['style'] : defaults.style
  return {
    style,
    barCount: number(value['barCount'], defaults.barCount, 16, 96),
    sensitivity: number(value['sensitivity'], defaults.sensitivity, 0.5, 4),
    smoothing: number(value['smoothing'], defaults.smoothing, 0, 0.95),
    primaryColor: string(value['primaryColor'], defaults.primaryColor),
    secondaryColor: string(value['secondaryColor'], defaults.secondaryColor),
    backgroundColor: string(value['backgroundColor'], defaults.backgroundColor),
    backgroundOpacity: number(value['backgroundOpacity'], defaults.backgroundOpacity, 0, 1),
    glow: number(value['glow'], defaults.glow, 0, 48),
    cornerRadius: number(value['cornerRadius'], defaults.cornerRadius, 0, 64),
  }
}

function isAudioFrame(value: Record<string, JsonValue>): boolean {
  return value['kind'] === 'audio.frame' && typeof value['capturedAtUnixMs'] === 'number'
    && typeof value['level'] === 'number' && typeof value['peak'] === 'number'
    && Array.isArray(value['bands']) && value['bands'].every((entry) => typeof entry === 'number')
}

function isRecord(value: JsonValue | undefined): value is Record<string, JsonValue> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function number(value: JsonValue | undefined, fallback: number, min: number, max: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? clamp(value, min, max) : fallback
}

function string(value: JsonValue | undefined, fallback: string): string {
  return typeof value === 'string' ? value : fallback
}

function clamp(value: number, min: number, max: number): number { return Math.min(max, Math.max(min, value)) }
