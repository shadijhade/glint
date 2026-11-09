import { Channel, invoke } from "@tauri-apps/api/core";
import type { FileEntry } from "./entries";
import { isArchived, isClosed, isOnlineOnly, isRecycled } from "./entries";
import { type ActionOutput, type ActionProgress, runAction } from "./actions";

/** Media studio (media_tools.rs): what FFmpeg trims, converts and reads. */
export const VIDEO_EXTS = new Set(["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "ts", "mts", "m2ts", "mpg", "mpeg", "3gp", "ogv", "vob", "asf", "f4v"]);
export const AUDIO_EXTS = new Set(["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "amr", "aiff", "aif", "ape", "mka", "ac3", "dts", "caf"]);

const onDisk = (e: FileEntry) => !e.dir && !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e);
export const isVideoFile = (e: FileEntry) => onDisk(e) && VIDEO_EXTS.has(e.ext);
export const isAudioFile = (e: FileEntry) => onDisk(e) && AUDIO_EXTS.has(e.ext);
export const isMediaFile = (e: FileEntry) => isVideoFile(e) || isAudioFile(e);

export interface Probe {
    duration: number;
    width: number;
    height: number;
    fps: number;
    video: string;
    audio: string;
    container: string;
    bitrate: number;
    playable: boolean;
}

export interface Encoders {
    h264: string | null;
    hevc: string | null;
    av1: string | null;
    webp: boolean;
    hardware: string[];
}

export type VideoFormat = "h264" | "hevc" | "av1";
export type Quality = { kind: "good" } | { kind: "high" } | { kind: "size"; mb: number };
export type AudioFormat = "same" | "mp3" | "m4a" | "opus" | "flac" | "wav";

export type MediaTask =
    | { kind: "trim"; start: number; end: number; exact: boolean }
    | { kind: "convert"; format: VideoFormat; quality: Quality; height: number | null }
    | { kind: "audio"; format: AudioFormat }
    | { kind: "gif"; width: number; fps: number; start: number | null; end: number | null; webp: boolean }
    | { kind: "subtitles"; translate: boolean }
    | { kind: "loudness"; music: boolean };

export const mediaProbe = (path: string) => invoke<Probe>("media_probe", { path });
export const mediaEncoders = () => invoke<Encoders>("media_encoders");
export const mediaCancel = () => invoke("media_cancel");
export const mediaRun = (files: string[], task: MediaTask, output: ActionOutput, onProgress: (p: ActionProgress) => void) => runAction("media_run", { files, task, output }, onProgress);

/** A copy of a video WebView2 can play (remuxed, or converted when its codec is the problem): its path. */
export function mediaPlayable(path: string, onProgress: (part: number) => void) {
    const channel = new Channel<number>();
    channel.onmessage = onProgress;
    return invoke<string>("media_playable", { path, onProgress: channel });
}

/** What a hardware encoder is called, for people. */
export function encoderName(encoder: string | null) {
    if (!encoder) return "";
    if (encoder.endsWith("_nvenc")) return "NVIDIA GPU";
    if (encoder.endsWith("_qsv")) return "Intel Quick Sync";
    if (encoder.endsWith("_amf")) return "AMD GPU";
    return { libx264: "x264", libx265: "x265", libsvtav1: "SVT-AV1" }[encoder] ?? encoder;
}
