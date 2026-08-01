import { t } from '../localization/i18n.js';

const OPERATION_STATUS_KEYS = {
    installing: 'status.installing',
    verifying: 'status.verifying_files',
    'updating-game': 'status.checking_game_updates',
    'launching-game': 'status.launching_game',
    'updating-launcher': 'status.downloading_launcher_update',
    'recovering-content': 'status.recovering_install',
    'installing-prerequisites': 'status.prerequisite_detecting'
};

const PREREQUISITE_STATUS_KEYS = {
    detecting: 'status.prerequisite_detecting',
    downloading: 'status.prerequisite_downloading',
    verifying: 'status.prerequisite_verifying',
    installing: 'status.prerequisite_installing',
    complete: 'status.prerequisite_verifying'
};

function formatEta(seconds, translate) {
    if (seconds == null) return '';
    if (seconds >= 3600) return `${Math.floor(seconds / 3600)} ${translate('time_units.hours')}`;
    if (seconds >= 60) return `${Math.floor(seconds / 60)} ${translate('time_units.minutes')}`;
    return `${Math.max(0, Math.floor(seconds))} ${translate('time_units.seconds')}`;
}

export function createStatusController({
    root = document.body,
    statusElement,
    progressBar,
    progressInfo,
    translate = t,
    renderActions = () => {}
}) {
    const settledOperationIds = new Set();
    let state = {
        kind: 'initializing',
        flow: 'startup',
        step: 'initializing',
        statusKey: 'status.loading_data',
        progress: null,
        progressStage: null,
        etaSec: null,
        indeterminate: true,
        activeOperationId: null
    };

    function render() {
        if (statusElement) {
            statusElement.textContent = translate(state.statusKey);
            statusElement.dataset.statusKind = state.kind;
        }
        if (progressBar) {
            progressBar.style.width = state.progress == null ? '0%' : `${state.progress}%`;
            progressBar.classList.toggle('indeterminate', state.kind === 'busy' && state.indeterminate);
        }
        if (progressInfo) {
            if (state.progress == null) {
                progressInfo.textContent = '';
            } else {
                const percentage = Math.floor(state.progress).toString().padStart(2, '0');
                const eta = formatEta(state.etaSec, translate);
                progressInfo.textContent = eta
                    ? `${percentage}% ${translate('time_units.remaining')}: ${eta}`
                    : `${percentage}%`;
            }
        }
        root?.setAttribute('aria-busy', state.kind === 'busy' || state.kind === 'initializing'
            ? 'true'
            : 'false');
        renderActions({ ...state });
    }

    function settleActiveOperation() {
        if (state.activeOperationId) settledOperationIds.add(state.activeOperationId);
    }

    function begin({ flow, step, statusKey, indeterminate = true, operationId = null }) {
        settleActiveOperation();
        state = {
            ...state,
            kind: 'busy', flow, step, statusKey, indeterminate,
            progress: null, progressStage: null, etaSec: null, activeOperationId: operationId
        };
        render();
    }

    function beginStep({ step, statusKey, indeterminate = true, operationId = null }) {
        settleActiveOperation();
        state = {
            ...state,
            kind: 'busy', step, statusKey, indeterminate,
            progress: null, progressStage: null, etaSec: null, activeOperationId: operationId
        };
        render();
    }

    function updateBusy({ step = state.step, statusKey, indeterminate = state.indeterminate }) {
        state = { ...state, kind: 'busy', step, statusKey, indeterminate };
        render();
    }

    function applyProgress(payload, statusKey) {
        if (!payload || state.kind !== 'busy') return false;
        const operationId = payload.operationId;
        if (operationId && settledOperationIds.has(operationId)) return false;
        if (state.activeOperationId && operationId && state.activeOperationId !== operationId) {
            return false;
        }

        const stageChanged = state.progressStage !== payload.stage;
        const numericProgress = Number.isFinite(payload.progress)
            ? Math.max(0, Math.min(100, payload.progress))
            : null;
        const progress = numericProgress == null
            ? state.progress
            : stageChanged ? numericProgress : Math.max(state.progress ?? 0, numericProgress);
        state = {
            ...state,
            statusKey: statusKey || state.statusKey,
            progress,
            progressStage: payload.stage || state.progressStage,
            etaSec: payload.timeRemainingSec ?? null,
            indeterminate: numericProgress == null,
            activeOperationId: state.activeOperationId || operationId || null
        };
        render();
        return true;
    }

    function finish(kind, statusKey) {
        settleActiveOperation();
        state = {
            ...state,
            kind,
            statusKey,
            progress: null,
            progressStage: null,
            etaSec: null,
            indeterminate: false,
            activeOperationId: null
        };
        render();
    }

    function setIdle(statusKey = 'status.ready') {
        finish('idle', statusKey);
    }

    function setRunning(statusKey = 'status.game_running') {
        finish('running', statusKey);
    }

    function restoreOperation(operation) {
        if (!operation || operation === 'idle') return false;
        begin({
            flow: 'restore',
            step: operation,
            statusKey: OPERATION_STATUS_KEYS[operation] || 'status.operation_in_progress'
        });
        return true;
    }

    function restorePrerequisite(snapshot) {
        if (!snapshot?.active) return false;
        const stage = snapshot.stage || 'detecting';
        begin({
            flow: 'prerequisites',
            step: stage,
            statusKey: PREREQUISITE_STATUS_KEYS[stage] || PREREQUISITE_STATUS_KEYS.detecting,
            indeterminate: !Number.isFinite(snapshot.progress),
            operationId: snapshot.operationId || null
        });
        applyProgress({
            operationId: snapshot.operationId,
            stage,
            progress: snapshot.progress,
            downloadedBytes: snapshot.downloadedBytes,
            totalBytes: snapshot.totalBytes
        }, PREREQUISITE_STATUS_KEYS[stage]);
        return true;
    }

    function rerender() {
        render();
    }

    function onLanguageChanged() {
        rerender();
    }

    function onLauncherClosing() {
        begin({ flow: 'close', step: 'closing', statusKey: 'status.closing' });
    }

    window.addEventListener('language-changed', onLanguageChanged);
    window.addEventListener('launcher-closing', onLauncherClosing);
    render();

    return {
        begin,
        beginStep,
        updateBusy,
        applyProgress,
        succeed: (statusKey) => finish('success', statusKey),
        fail: (statusKey) => finish('error', statusKey),
        cancel: (statusKey) => finish('canceled', statusKey),
        setIdle,
        setRunning,
        restoreOperation,
        restorePrerequisite,
        rerender,
        getState: () => ({ ...state }),
        dispose() {
            window.removeEventListener('language-changed', onLanguageChanged);
            window.removeEventListener('launcher-closing', onLauncherClosing);
        }
    };
}

export function createOperationId() {
    if (globalThis.crypto?.randomUUID) return globalThis.crypto.randomUUID();
    return `00000000-0000-4000-8000-${Date.now().toString(16).padStart(12, '0').slice(-12)}`;
}
