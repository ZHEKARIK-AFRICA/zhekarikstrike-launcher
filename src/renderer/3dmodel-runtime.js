export const BASE_SPIN_DURATION_MS = 1400 / 1.5;

export function advanceSpin(progress, elapsedMs, speedMultiplier = 1) {
    if (!Number.isFinite(progress) || !Number.isFinite(elapsedMs)) return progress;
    const increment = Math.max(0, elapsedMs) * Math.max(0, speedMultiplier) / BASE_SPIN_DURATION_MS;
    return Math.min(1, Math.max(0, progress) + increment);
}

export function createPointerCoalescer({ handle }) {
    let pendingEvent = null;

    function flush(timestamp) {
        const event = pendingEvent;
        pendingEvent = null;
        if (event) handle(event, timestamp);
    }

    return {
        push(event) {
            pendingEvent = { clientX: event.clientX, clientY: event.clientY };
        },
        flush,
        cancel() {
            pendingEvent = null;
        }
    };
}

export async function loadSequentially(urls, load, onLoaded = () => {}) {
    const loaded = [];
    for (const url of urls) {
        const model = await load(url);
        loaded.push(model);
        onLoaded(model);
    }
    return loaded;
}

export function createAnimationDriver({ requestFrame, cancelFrame, onFrame }) {
    let visible = true;
    let running = false;
    let frameId = null;
    let previousTimestamp = null;

    function frame(timestamp) {
        frameId = null;
        if (!running || !visible) return;
        const elapsed = previousTimestamp == null ? 0 : Math.max(0, timestamp - previousTimestamp);
        previousTimestamp = timestamp;
        onFrame(elapsed, timestamp);
        frameId = requestFrame(frame);
    }

    function schedule() {
        if (running && visible && frameId == null) frameId = requestFrame(frame);
    }

    return {
        start() {
            if (running) return;
            running = true;
            previousTimestamp = null;
            schedule();
        },
        stop() {
            running = false;
            previousTimestamp = null;
            if (frameId != null) cancelFrame(frameId);
            frameId = null;
        },
        setVisible(nextVisible) {
            visible = Boolean(nextVisible);
            previousTimestamp = null;
            if (!visible && frameId != null) {
                cancelFrame(frameId);
                frameId = null;
            }
            schedule();
        }
    };
}
