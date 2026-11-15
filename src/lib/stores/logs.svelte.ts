import { appendLogs, type LogEntry, type LogLevel } from '../logs';

/** Log ring shared by the console panel and the rest of the app. */
export class LogStore {
    logs = $state<LogEntry[]>([]);

    add(lines: string | string[], level?: LogLevel) {
        this.logs = appendLogs(this.logs, lines, level);
    }

    clear() {
        this.logs = [];
    }

    set(next: LogEntry[] | ((prev: LogEntry[]) => LogEntry[])) {
        this.logs = typeof next === 'function' ? next(this.logs) : next;
    }
}
