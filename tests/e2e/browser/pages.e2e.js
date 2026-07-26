import { $, browser, expect } from '@wdio/globals';

const baseUrl = 'http://127.0.0.1:5173';

async function openWithMocks(page, definitions) {
    await browser.url(`${baseUrl}/public/${page}`);
    await browser.tauri.restoreAllMocks();
    const mocks = {};
    for (const [command, definition] of Object.entries(definitions)) {
        const mock = await browser.tauri.mock(command);
        if (definition?.reject !== undefined) {
            await mock.mockRejectedValue(definition.reject);
        } else {
            await mock.mockResolvedValue(definition?.value);
        }
        mocks[command] = mock;
    }
    await browser.execute(() => {
        window.__ZHEKARIK_E2E_READY__ = true;
    });
    return mocks;
}

async function expectModalMessage(text) {
    const modal = await $('#error-modal');
    await browser.waitUntil(async () => (await modal.getCSSProperty('display')).value === 'block');
    await expect($('#error-message')).toHaveText(expect.stringContaining(text));
}

const installStartup = {
    get_language: { value: 'en' },
    get_game_path: { value: 'D:\\Games\\ZHEKARIKSTRIKE' }
};

const mainStartup = {
    get_language: { value: 'en' },
    get_game_data: {
        value: {
            nickname: 'Player',
            clanTag: 'ZS',
            launchParams: '-novid',
            gamePath: 'D:\\Games\\ZHEKARIKSTRIKE'
        }
    },
    get_current_state: {
        value: { processInProgress: false, verificationInProgress: false, operation: 'idle' }
    }
};

describe('Tauri renderer pages in browser mode', () => {
    it('loads intro, handles its event, and unsubscribes on pagehide', async () => {
        await browser.url(`${baseUrl}/public/intro.html`);
        await browser.tauri.restoreAllMocks();
        const layout = await browser.tauri.mock('set_window_layout');
        await layout.mockRejectedValue({ code: 'window', message: 'resize failed' });
        const unlisten = await browser.tauri.mock('plugin:event|unlisten');

        await browser.tauri.emitEvent('start-fade-out', './public/install.html');
        await expect($('body')).toHaveElementClass('fade-out');
        await browser.execute(() => window.dispatchEvent(new Event('pagehide')));
        await unlisten.update();
        expect(unlisten.mock.calls.length).toBeGreaterThan(0);
        await browser.execute(() => document.body.dispatchEvent(new Event('animationend')));
        await browser.waitUntil(async () => (await browser.getUrl()).endsWith('/public/install.html'));
    });

    it('prefills the install page and completes installation from invoke result', async () => {
        const mocks = await openWithMocks('install.html', {
            ...installStartup,
            install_game: { value: null },
            set_window_layout: { value: null }
        });
        await mocks.install_game.mockImplementation((args) => {
            localStorage.setItem('lastInstallArgs', JSON.stringify(args));
        });

        await expect($('#install-path')).toHaveValue('D:\\Games\\ZHEKARIKSTRIKE');
        await $('#start-install').click();
        await browser.waitUntil(async () => (await browser.getUrl()).endsWith('/public/index.html'));
        const args = await browser.execute(() => JSON.parse(localStorage.getItem('lastInstallArgs')));
        expect(args).toEqual({ gamePath: 'D:\\Games\\ZHEKARIKSTRIKE' });
    });

    it('renders install errors and cancellation without terminal events', async () => {
        await openWithMocks('install.html', {
            ...installStartup,
            install_game: { reject: { code: 'network', message: 'offline' } }
        });
        await $('#start-install').click();
        await expectModalMessage('offline');

        await openWithMocks('install.html', {
            ...installStartup,
            install_game: { reject: { code: 'canceled', message: 'Operation canceled' } },
            cancel_install: { value: null }
        });
        await $('#start-install').click();
        await expect($('#install-status')).toHaveText('cancel');
        await $('#cancel-install').click();
    });

    it('passes true for manual verify and handles success, error, and cancel', async () => {
        let mocks = await openWithMocks('index.html', {
            ...mainStartup,
            verify_files: { value: null }
        });
        await $('#check-files').click();
        await mocks.verify_files.update();
        expect(mocks.verify_files.mock.calls[0][0]).toEqual({ checkAllFiles: true });
        await expect($('#launcher-status')).toHaveText('files are good!');

        await openWithMocks('index.html', {
            ...mainStartup,
            verify_files: { reject: { code: 'network', message: 'verify failed' } }
        });
        await $('#check-files').click();
        await expectModalMessage('verify failed');

        mocks = await openWithMocks('index.html', {
            ...mainStartup,
            verify_files: { reject: { code: 'canceled', message: 'Operation canceled' } }
        });
        await $('#check-files').click();
        await mocks.verify_files.update();
        await expect($('#launcher-status')).toHaveText('verification canceled');
    });

    it('runs the pre-launch chain with false verify and camelCase rev.ini args', async () => {
        const mocks = await openWithMocks('index.html', {
            ...mainStartup,
            update_game: { value: null },
            verify_files: { value: null },
            update_rev_ini: { value: null },
            launch_game: { value: null }
        });

        await $('#play-button').click();
        await browser.waitUntil(async () => {
            await mocks.launch_game.update();
            return mocks.launch_game.mock.calls.length === 1;
        });
        await mocks.verify_files.update();
        await mocks.update_rev_ini.update();
        expect(mocks.verify_files.mock.calls[0][0]).toEqual({ checkAllFiles: false });
        expect(mocks.update_rev_ini.mock.calls[0][0]).toEqual({
            launchParams: '-novid',
            clanTag: 'ZS',
            nickname: 'Player'
        });
    });

    it('shows launch errors returned by invoke', async () => {
        await openWithMocks('index.html', {
            ...mainStartup,
            update_game: { reject: { code: 'network', message: 'update unavailable' } }
        });
        await $('#play-button').click();
        await expectModalMessage('update unavailable');
    });

    it('handles updater success and allows continuing after a damaged artifact', async () => {
        let mocks = await openWithMocks('launcher_update.html', {
            get_language: { value: 'en' },
            download_launcher_update: { value: null },
            apply_launcher_update: { value: null }
        });
        await browser.waitUntil(async () => {
            await mocks.apply_launcher_update.update();
            return mocks.apply_launcher_update.mock.calls.length === 1;
        });

        await openWithMocks('launcher_update.html', {
            get_language: { value: 'en' },
            download_launcher_update: {
                reject: { code: 'invalid-data', message: 'signature mismatch' }
            }
        });
        await expectModalMessage('signature mismatch');
        await expect($('#continue-without-update')).toBeDisplayed();
    });
});
