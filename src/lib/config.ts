import { MIRROR_AUDIO_SOURCES } from './types';
import type {
    MicSource,
    MirrorAudioSource,
    ScrcpyConfig,
    SessionMode
} from './types';

export const DEFAULT_CONFIG: ScrcpyConfig = {
    device: '',
    sessionMode: 'mirror',
    bitrate: 8,
    fps: undefined,
    stayAwake: false,
    turnOff: false,
    audioEnabled: true,
    audioCodec: 'auto',
    mirrorAudioSource: 'output',
    audioDup: false,
    autoReconnect: false,
    micSetDefaultSource: false,
    requireAudio: false,
    alwaysOnTop: false,
    res: '0',
    recordPath: '',
    vdWidth: 1920,
    vdHeight: 1080,
    vdDpi: 420,
    aspectRatioLock: true,
    hidKeyboard: false,
    hidMouse: false,
    flexDisplay: false,
    cameraTorch: false,
    cameraZoom: 1.0,
    cameraMicSink: false,
    backgroundColor: '',
    keepActive: false,
    vsync: true,
    rememberWindowPosition: true,
    ignoreVideoEncoderConstraints: false
};

const SESSION_MODES: SessionMode[] = ['mirror', 'camera', 'desktop', 'mic'];
const AUDIO_CODECS = ['auto', 'opus', 'aac', 'flac', 'raw'] as const;
const MIC_SOURCES = ['mic', 'mic-voice-communication'] as const;
const MIC_CODECS = ['opus', 'aac'] as const;
const ROTATIONS = ['0', '90', '180', '270'] as const;
const CAMERA_FACINGS = ['front', 'back', 'external'] as const;

// Persisted config is a trust boundary: localStorage can be stale or edited.
// These lists keep the runtime shape of the config aligned with its type, so
// an invalid entry cannot reach Svelte bindings or the Tauri command payload.
const BOOLEAN_FIELDS = [
    'stayAwake',
    'turnOff',
    'audioEnabled',
    'audioDup',
    'autoReconnect',
    'requireAudio',
    'alwaysOnTop',
    'fullscreen',
    'borderless',
    'record',
    'otgPure',
    'cameraHighSpeed',
    'cameraTorch',
    'cameraMicSink',
    'hidKeyboard',
    'hidMouse',
    'flexDisplay',
    'keepActive',
    'vsync',
    'rememberWindowPosition',
    'ignoreVideoEncoderConstraints',
    'aspectRatioLock',
    'micDenoise',
    'micSetDefaultSource'
] as const;

const NUMBER_FIELDS = [
    'bitrate',
    'fps',
    'vdWidth',
    'vdHeight',
    'vdDpi',
    'cameraZoom',
    'micBitrate',
    'windowX',
    'windowY'
] as const;

const STRING_FIELDS = [
    'device',
    'recordPath',
    'scrcpyPath',
    'cameraId',
    'cameraAr',
    'backgroundColor',
    'res',
    'renderDriver',
    'codec'
] as const;

function resetInvalidFields<K extends keyof ScrcpyConfig>(
    config: ScrcpyConfig,
    input: Partial<ScrcpyConfig>,
    fields: readonly K[],
    isValid: (value: unknown) => boolean
): void {
    for (const field of fields) {
        if (!Object.prototype.hasOwnProperty.call(input, field)) continue;
        if (!isValid(input[field])) config[field] = DEFAULT_CONFIG[field];
    }
}

/** Persisted config is a trust boundary: localStorage can be stale or edited.
 *  Normalize enums before the value reaches the store/backend. */
export function normalizeConfig(raw: Partial<ScrcpyConfig> | null): ScrcpyConfig {
    const input =
        raw && typeof raw === 'object' && !Array.isArray(raw) ? raw : {};
    const config: ScrcpyConfig = { ...DEFAULT_CONFIG, ...input };

    resetInvalidFields(config, input, BOOLEAN_FIELDS, value => typeof value === 'boolean');
    resetInvalidFields(
        config,
        input,
        NUMBER_FIELDS,
        value => typeof value === 'number' && Number.isFinite(value)
    );
    resetInvalidFields(config, input, STRING_FIELDS, value => typeof value === 'string');

    if (!SESSION_MODES.includes(config.sessionMode)) config.sessionMode = 'mirror';
    if (!config.audioCodec || !AUDIO_CODECS.includes(config.audioCodec)) config.audioCodec = 'auto';
    if (!MIRROR_AUDIO_SOURCES.includes(config.mirrorAudioSource as MirrorAudioSource)) {
        config.mirrorAudioSource = 'output';
    }
    // --audio-dup is only valid with --audio-source=playback.
    if (config.mirrorAudioSource !== 'playback') config.audioDup = false;

    // Older builds persisted "voice-communication"; the raw value can be any
    // string because localStorage is not typed.
    const rawMicSource = (input as { micSource?: unknown }).micSource;
    if (rawMicSource === 'voice-communication') config.micSource = 'mic-voice-communication';
    else if (rawMicSource !== undefined && !MIC_SOURCES.includes(rawMicSource as MicSource)) {
        config.micSource = 'mic';
    }
    if (config.micCodec && !MIC_CODECS.includes(config.micCodec)) config.micCodec = 'opus';
    if (config.rotation && !ROTATIONS.includes(config.rotation)) config.rotation = '0';
    if (config.cameraFacing && !CAMERA_FACINGS.includes(config.cameraFacing)) {
        config.cameraFacing = undefined;
    }
    return config;
}
