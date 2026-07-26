import en from '../localization/locales/en.json';
import ru from '../localization/locales/ru.json';

const translations = { en, ru };
let currentLanguage = 'ru';

const invokeMap = {
    'apply-launcher-update': 'apply_launcher_update',
    'cancel-install': 'cancel_install',
    'cancel-verify': 'cancel_verify',
    'check-game-exists': 'check_game_exists',
    'check-disk-space-for-install': 'check_disk_space_for_install',
    'check-launcher-update': 'check_launcher_update',
    'close-window': 'close_window',
    'create-shortcuts': 'create_shortcuts',
    'download-launcher-update': 'download_launcher_update',
    'get-config': 'get_config',
    'get-current-state': 'get_current_state',
    'get-game-data': 'get_game_data',
    'get-game-path': 'get_game_path',
    'get-game-process-state': 'get_game_process_state',
    'get-game-version': 'get_game_version',
    'get-language': 'get_language',
    'get-startup-state': 'get_startup_state',
    'install-game': 'install_game',
    'is-elevated': 'is_elevated',
    'launch-game': 'launch_game',
    'minimize-window': 'minimize_window',
    'move-launcher-to-game-path': 'move_launcher_to_game_path',
    'select-folder': 'select_game_folder',
    'select-game-folder': 'select_game_folder',
    'relaunch-as-admin': 'relaunch_as_admin',
    'set-window-layout': 'set_window_layout',
    'set-game-path': 'set_game_path',
    'set-language': 'set_language',
    'show-main-window': 'show_main_window',
    'stop-game': 'stop_game',
    'translate': 'translate',
    'update-game': 'update_game',
    'update-rev-ini': 'update_rev_ini',
    'verify-files': 'verify_files'
};

const eventMap = {
    'launch-error': 'game-error',
    'update-progress': 'launcher-update-progress'
};

function getTauriApi() {
    if (window.__TAURI__) {
        return window.__TAURI__;
    }

    return {
        core: {
            invoke: async (command, args = {}) => {
                console.warn(`[tauri mock] invoke ${command}`, args);
                if (command === 'get_language') return currentLanguage;
                if (command === 'set_language') {
                    currentLanguage = args.language || currentLanguage;
                    return null;
                }
                if (command === 'get_game_data') {
                    return { nickname: '', clanTag: '', launchParams: '', gamePath: '' };
                }
                if (command === 'get_current_state') {
                    return { ProcessInProgress: false, processInProgress: false, verificationInProgress: false };
                }
                if (command === 'get_game_path') return null;
                if (command === 'get_game_version') return '0.0.0';
                if (command === 'select_game_folder') return null;
                return null;
            }
        },
        event: {
            listen: async () => () => {}
        }
    };
}

function toCamelCase(name) {
    return name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

function mapPayload(channel, payload) {
    if (payload == null) {
        return {};
    }

    if (channel === 'install-game') {
        return { gamePath: payload };
    }

    if (channel === 'set-language') {
        return { language: payload };
    }

    if (channel === 'set-game-path') {
        return { gamePath: payload };
    }

    if (channel === 'verify-files') {
        return { checkAllFiles: payload };
    }

    if (channel === 'translate') {
        return { key: payload };
    }

    if (typeof payload !== 'object' || Array.isArray(payload)) {
        return { value: payload };
    }

    return Object.entries(payload).reduce((result, [key, value]) => {
        result[toCamelCase(key)] = value;
        return result;
    }, {});
}

function normalizePage(page) {
    const normalized = String(page || '').replaceAll('\\', '/');
    const prefix = window.location.pathname.includes('/public/') ? '/public' : '';

    if (normalized.endsWith('intro.html')) return `${prefix}/intro.html`;
    if (normalized.endsWith('install.html')) return `${prefix}/install.html`;
    if (normalized.endsWith('launcher_update.html')) return `${prefix}/launcher_update.html`;
    if (normalized.endsWith('index.html')) return `${prefix}/index.html`;

    return normalized;
}

function getNestedTranslation(key) {
    const dictionary = translations[currentLanguage] || translations.ru;
    return key.split('.').reduce((value, part) => {
        if (value && Object.prototype.hasOwnProperty.call(value, part)) {
            return value[part];
        }
        return undefined;
    }, dictionary);
}

async function translate(key) {
    return getNestedTranslation(key) ?? key;
}

async function invoke(channel, payload) {
    if (channel === 'translate') {
        return translate(payload);
    }

    const command = invokeMap[channel] || channel;
    const args = mapPayload(channel, payload);
    try {
        return await getTauriApi().core.invoke(command, args);
    } catch (error) {
        throw normalizeError(error);
    }
}

async function listen(channel, callback) {
    const eventName = eventMap[channel] || channel;
    return getTauriApi().event.listen(eventName, (event) => {
        callback(null, normalizeEventPayload(channel, event.payload));
    });
}

function normalizeError(error) {
    if (error instanceof Error) {
        return error;
    }

    const message = frontendErrorMessage(error);
    const normalized = new Error(message);
    if (error && typeof error === 'object') {
        normalized.code = error.code;
        normalized.details = error.details;
    }
    return normalized;
}

function frontendErrorMessage(payload) {
    if (payload == null) return '';
    if (typeof payload === 'string') return payload;
    if (typeof payload === 'object') {
        return payload.message || payload.details || JSON.stringify(payload);
    }
    return String(payload);
}

function normalizeEventPayload(channel, payload) {
    if (payload && typeof payload === 'object' && !Array.isArray(payload)) {
        const normalized = { ...payload };
        if ('timeRemainingSec' in normalized && !('timeRemaining' in normalized)) {
            normalized.timeRemaining = normalized.timeRemainingSec;
        }
        if ('message' in normalized && !('errorMessage' in normalized)) {
            normalized.errorMessage = normalized.message;
        }
        if ('code' in normalized && 'message' in normalized) {
            return frontendErrorMessage(normalized);
        }
        return normalized;
    }

    return payload;
}

async function setLanguage(language) {
    currentLanguage = language || 'ru';
    await invoke('set-language', currentLanguage);
    window.dispatchEvent(new CustomEvent('language-changed', { detail: currentLanguage }));
}

async function getLanguage() {
    currentLanguage = await invoke('get-language');
    return currentLanguage;
}

document.addEventListener('DOMContentLoaded', () => {
    document.querySelectorAll('#drag-bar, [data-tauri-drag-region]').forEach((element) => {
        element.setAttribute('data-tauri-drag-region', '');
    });
    setupInputContextMenu();
    listen('global-error', (_event, message) => showGlobalError(message));
});

function showGlobalError(message) {
    const errorModal = document.getElementById('error-modal');
    const errorMessage = document.getElementById('error-message');
    if (errorModal && errorMessage) {
        errorMessage.textContent = frontendErrorMessage(message);
        errorModal.style.display = 'flex';
        return;
    }
    console.error('Global error:', message);
}

function setupInputContextMenu() {
    let menu = document.getElementById('tauri-input-context-menu');
    if (!menu) {
        menu = document.createElement('div');
        menu.id = 'tauri-input-context-menu';
        Object.assign(menu.style, {
            position: 'fixed',
            display: 'none',
            zIndex: '10000',
            background: '#1f1f1f',
            border: '1px solid #444',
            color: '#fff',
            fontSize: '12px',
            padding: '4px',
            boxShadow: '0 6px 18px rgba(0,0,0,.35)'
        });
        document.body.appendChild(menu);
    }

    document.addEventListener('click', () => {
        menu.style.display = 'none';
    });

    document.addEventListener('contextmenu', (event) => {
        const target = event.target;
        const editable = target instanceof HTMLInputElement
            || target instanceof HTMLTextAreaElement
            || target?.isContentEditable;
        if (!editable) return;

        event.preventDefault();
        menu.replaceChildren(
            contextButton('Cut', () => document.execCommand('cut')),
            contextButton('Copy', () => document.execCommand('copy')),
            contextButton('Paste', async () => {
                const text = await navigator.clipboard?.readText?.();
                if (text) document.execCommand('insertText', false, text);
            }),
            contextButton('Select All', () => {
                if (typeof target.select === 'function') target.select();
                else document.execCommand('selectAll');
            })
        );
        menu.style.left = `${event.clientX}px`;
        menu.style.top = `${event.clientY}px`;
        menu.style.display = 'block';
    });
}

function contextButton(label, action) {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = label;
    Object.assign(button.style, {
        display: 'block',
        width: '100%',
        padding: '5px 14px',
        color: '#fff',
        background: 'transparent',
        border: '0',
        textAlign: 'left',
        cursor: 'default'
    });
    button.addEventListener('click', async (event) => {
        event.stopPropagation();
        await action();
        document.getElementById('tauri-input-context-menu').style.display = 'none';
    });
    return button;
}

window.electronAPI = {
    invoke,
    on: listen,
    send: (channel, payload) => invoke(channel, payload),
    navigateToPage: (page) => {
        invoke('set-window-layout', { page }).finally(() => {
            window.location.href = normalizePage(page);
        });
    },
    openExternal: (url) => invoke('open_external_url', { url }),
    minimizeWindow: () => invoke('minimize-window'),
    closeWindow: () => invoke('close-window'),
    getLanguage,
    setLanguage,
    t: translate,
    loadTranslations: async (language) => {
        currentLanguage = language || currentLanguage;
        return true;
    },
    onLanguageChanged: (callback) => {
        window.addEventListener('language-changed', (event) => callback(event.detail));
    }
};
