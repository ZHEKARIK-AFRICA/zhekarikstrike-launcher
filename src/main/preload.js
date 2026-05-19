// preload.js
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('electronAPI', {
    getLanguage: () => ipcRenderer.invoke('get-language'),
    setLanguage: (lang) => ipcRenderer.invoke('set-language', lang),
    invoke: (channel, data) => ipcRenderer.invoke(channel, data),
    on: (channel, func) => ipcRenderer.on(channel, (event, ...args) => func(event, ...args)),
    navigateToPage: (page) => ipcRenderer.send('navigate-to-page', page),
    openExternal: (url) => ipcRenderer.send('open-external', url),
    send: (channel, data) => ipcRenderer.send(channel, data),
    minimizeWindow: () => ipcRenderer.send('minimize-window'),
    maximizeWindow: () => ipcRenderer.send('maximize-window'),
    closeWindow: () => ipcRenderer.send('close-window'),
    t: (key) => ipcRenderer.invoke('translate', key),
    setLanguage: (lang) => ipcRenderer.invoke('set-language', lang),
    loadTranslations: (lang) => ipcRenderer.invoke('load-translations', lang),
    onLanguageChanged: (callback) => ipcRenderer.on('language-changed', (event, lang) => callback(lang)),
});