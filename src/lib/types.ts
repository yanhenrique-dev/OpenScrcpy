export interface RenderDriverOption {
    id: string;
    label: string;
}

export interface RenderDriverSupport {
    hostOs: string;
    supportsRenderDriver: boolean;
    supportedDrivers: RenderDriverOption[];
}

export interface MdnsDevice {
    name: string;
    service: string;
    address: string;
}

export function isMdnsDeviceConnected(dev: MdnsDevice, devices: string[]): boolean {
    const serials = [
        `${dev.name}._adb-tls-connect._tcp`,
        `${dev.name}._adb-tls-connect._tcp (2)`,
        `${dev.name}._adb-tls-connect._udp`,
        `${dev.name}._adb-tls-connect._udp (2)`,
    ];
    return devices.includes(dev.address) || devices.some(d => serials.includes(d));
}

export interface CommandResult {
    success: boolean;
    message?: string;
    output?: string;
    stdout?: string;
    stderr?: string;
    binary?: string;
}

export interface UnavailableDevice {
    serial: string;
    state: string;
    model?: string;
}

export interface DeviceListResult {
    error?: boolean;
    message?: string;
    devices?: string[];
    deviceModels?: Record<string, string>;
    unavailable?: UnavailableDevice[];
}

export interface MdnsDevicesResult {
    error?: boolean;
    message?: string;
    services?: MdnsDevice[];
}

export interface DownloadStatus {
    found: boolean;
    message: string;
}

export type SessionEndReason = 'stopped' | 'exited' | 'error';

/** Payload of the shared `scrcpy-status` event. Download progress and session
 *  state share the channel; only one group of fields is present at a time. */
export interface SessionStatusEvent {
    type?: 'downloading' | 'download-progress' | 'download-complete';
    percent?: number;
    device?: string;
    running?: boolean;
    reason?: SessionEndReason;
    /** Set when reason is "exited"; false means scrcpy failed or was killed. */
    success?: boolean;
}

export function mdnsHostIp(address: string): string {
    const value = address.trim();
    // Bracketed IPv6 keeps the brackets (the pairing flow matches addresses
    // by this key), with or without a port.
    if (value.startsWith('[')) {
        const close = value.indexOf(']');
        return close > 0 ? value.slice(0, close + 1) : value;
    }
    const lastColon = value.lastIndexOf(':');
    if (lastColon <= 0) return value;
    const host = value.slice(0, lastColon);
    // A bare IPv6 address has several colons and no port to strip; using the
    // last colon would truncate it to "fe80:".
    if (host.includes(':')) return value;
    return host;
}

export type WindowPositions = Record<string, { x: number; y: number }>;

export type SessionMode = 'mirror' | 'camera' | 'desktop' | 'mic';
export type AudioCodec = 'auto' | 'opus' | 'aac' | 'flac' | 'raw';
/** Reported by the backend as sessions start, hit audio trouble and stop. */
export type AudioStatus = 'ok' | 'warning' | 'error' | 'off';
export type MicSource = 'mic' | 'mic-voice-communication';
export type MicCodec = 'opus' | 'aac';
export type Rotation = '0' | '90' | '180' | '270';
export type ColorMode = 'light' | 'dark' | 'system';
export type CameraFacing = 'front' | 'back' | 'external';

export interface ScrcpyConfig {
    device: string;
    sessionMode: SessionMode;
    bitrate?: number;
    fps?: number;
    stayAwake?: boolean;
    turnOff?: boolean;
    audioEnabled?: boolean;
    audioCodec?: AudioCodec;
    /** Fail the session instead of silently mirroring video only when the
     *  device cannot capture audio. Off by default for older-device support. */
    requireAudio?: boolean;
    alwaysOnTop?: boolean;
    fullscreen?: boolean;
    borderless?: boolean;
    record?: boolean;
    recordPath?: string;
    scrcpyPath?: string;
    otgPure?: boolean;
    cameraFacing?: CameraFacing;
    cameraId?: string;
    codec?: string;
    cameraAr?: string;
    cameraHighSpeed?: boolean;
    vdWidth?: number;
    vdHeight?: number;
    vdDpi?: number;
    rotation?: Rotation;
    res?: string;
    aspectRatioLock?: boolean;
    hidKeyboard?: boolean;
    hidMouse?: boolean;
    renderDriver?: string;
    flexDisplay?: boolean;
    cameraTorch?: boolean;
    cameraZoom?: number;
    /** Linux: route the camera session's microphone into the OpenScrcpy Mic
     *  virtual sink so OBS/Discord can select it as a recording device. */
    cameraMicSink?: boolean;
    backgroundColor?: string;
    keepActive?: boolean;
    vsync?: boolean;
    rememberWindowPosition?: boolean;
    ignoreVideoEncoderConstraints?: boolean;
    windowX?: number;
    windowY?: number;
    micSource?: MicSource;
    micCodec?: MicCodec;
    micBitrate?: number;
    micDenoise?: boolean;
}

export type ConfigFieldSetter = <K extends keyof ScrcpyConfig>(
    field: K,
    value: ScrcpyConfig[K]
) => void;

export interface DeviceGroup {
    name: string;
    ids: string[];
}
