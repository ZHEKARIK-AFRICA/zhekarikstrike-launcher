import { $, browser, expect } from '@wdio/globals';

async function navigate(page) {
    await browser.url(`http://tauri.localhost/public/${page}`);
    await browser.waitUntil(async () => (await browser.getUrl()).endsWith(`/public/${page}`));
}

async function ready() {
    await browser.execute(() => {
        window.__ZHEKARIK_E2E_READY__ = true;
    });
}

describe('Windows Tauri E2E build', () => {
    it('starts on intro and reaches install', async () => {
        await expect($('#intro')).toBeDisplayed();
        await navigate('install.html');
        await ready();
        await expect($('#start-install')).toBeDisplayed();
    });

    it('covers installation and cancellation UI', async () => {
        await navigate('install.html');
        await ready();
        await expect($('#install-path')).toHaveValue('D:\\Games\\ZHEKARIKSTRIKE');
        await $('#start-install').click();
        await browser.waitUntil(async () => (await browser.getUrl()).endsWith('/public/index.html'));

        await navigate('install.html');
        await ready();
        await $('#install-path').setValue('D:\\cancel');
        await $('#start-install').click();
        await $('#cancel-install').waitForEnabled();
        await $('#cancel-install').click();
        await expect($('#install-status')).toHaveText('cancel');
    });

    it('covers verify, launch, and structured launch errors', async () => {
        await navigate('index.html');
        await ready();
        await $('#check-files').click();
        await expect($('#launcher-status')).toHaveText('files are good!');
        await $('#play-button').click();
        await $('#play-button').waitForEnabled();

        await navigate('index.html');
        await ready();
        await $('#nickname').setValue('error');
        await $('#play-button').click();
        await browser.waitUntil(async () =>
            (await $('#error-modal').getCSSProperty('display')).value === 'block'
        );
        await expect($('#error-message')).toHaveText(expect.stringContaining('native launch fixture failed'));
    });

    it('shows updater failure and continue UI', async () => {
        await navigate('launcher_update.html');
        await ready();
        await expect($('#continue-without-update')).toBeDisplayed();
        await expect($('#error-message')).toHaveText(expect.stringContaining('tampered native artifact'));
    });
});
