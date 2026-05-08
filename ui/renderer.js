const tauriApi = window.__TAURI__;
const invoke = tauriApi?.core?.invoke;
const listen = tauriApi?.event?.listen;
const convertFileSrc = tauriApi?.core?.convertFileSrc;
const confirmDialog = tauriApi?.dialog?.confirm;
const MarkdownIt = window.markdownit;
const DEFAULT_MASTER_PROMPT = `You are an expert summarizer. Summarize the following video concisely:

Title: {title}

Transcript:
{transcript}

Summary:`;
const DEFAULT_TRANSLATION_PROMPTS = {
  de: `Translate the following summary into German. Only output the translated summary, no explanation or intro. If it's already in the target language, do nothing but repeat it.

Summary:
{summary}

Translation:`,
  jp: `Translate the following summary into Japanese. Only output the translated summary, no explanation or intro. If it's already in the target language, do nothing but repeat it.

Summary:
{summary}

Translation:`
};
const YOUTUBE_COOKIE_SOURCE_KEY = 'youtubeCookieSource';
const YOUTUBE_COOKIE_ERROR_PATTERNS = [
  'sign in to confirm',
  'not a bot',
  'login_required',
  'cookies-from-browser',
  'cookies for the authentication',
  'could not load cookies',
  'failed to decrypt',
  'permission denied'
];

if (!invoke || !listen) {
  throw new Error('Tauri runtime API is unavailable.');
}

if (typeof MarkdownIt !== 'function') {
  throw new Error('markdown-it is unavailable.');
}

const markdownRenderer = createMarkdownRenderer();

function createMarkdownRenderer() {
  const renderer = new MarkdownIt({
    html: false,
    linkify: true,
    typographer: false,
    breaks: true
  });
  const defaultLinkOpen =
    renderer.renderer.rules.link_open ||
    ((tokens, idx, options, env, self) => self.renderToken(tokens, idx, options));

  renderer.renderer.rules.link_open = (tokens, idx, options, env, self) => {
    tokens[idx].attrJoin('class', 'summary-link');
    tokens[idx].attrSet('target', '_blank');
    tokens[idx].attrSet('rel', 'noopener noreferrer');
    return defaultLinkOpen(tokens, idx, options, env, self);
  };

  return renderer;
}

function preprocessLLMMarkdown(text) {
  let normalized = String(text || '')
    .replace(/<\/think(?:ing)?>[^\S\n]*\n+[^\S\n]*/gi, '</think>')
    .replace(
      /(^|\n)\s*<think(?:ing)?>[\s\S]*?(?:<\/think(?:ing)?>|$)\s*(\n\s*\n)?/gi,
      (_, lead) => (lead ? '\n' : '')
    )
    .replace(/\r\n/g, '\n')
    .replace(/\r/g, '\n')
    .replace(/[\u00a0\u202f\u2007]/g, ' ');

  return balanceStreamingCodeFence(normalized);
}

function markdownToHTML(text) {
  return markdownRenderer.render(preprocessLLMMarkdown(text));
}

function balanceStreamingCodeFence(markdown) {
  const lines = markdown.split(/\r?\n/);
  let open = null;

  for (const line of lines) {
    if (!open) {
      const match = line.match(/^\s*([`~]{3,})([^\s]*)?.*$/);
      if (match) {
        open = { fenceChar: match[1][0], fenceLen: match[1].length };
      }
      continue;
    }

    const closeFence = new RegExp(`^\\s*(${open.fenceChar}{${open.fenceLen},})\\s*$`);
    if (closeFence.test(line)) {
      open = null;
    }
  }

  if (!open) {
    return markdown;
  }

  const closingFence = open.fenceChar.repeat(open.fenceLen);
  return markdown.endsWith('\n') ? markdown + closingFence : `${markdown}\n${closingFence}`;
}

function toWebviewFileUrl(filePath) {
  if (!filePath) {
    return filePath;
  }
  if (typeof convertFileSrc === 'function') {
    return convertFileSrc(filePath);
  }
  return filePath;
}

window.api = {
  getModels: () => invoke('get_models'),
  getSummaries: () => invoke('get_summaries'),
  getYoutubeCookieSources: () => invoke('get_youtube_cookie_sources'),
  summarizeVideo: (url, useWhisper, model, masterPrompt, cookieSource) => invoke('summarize_video', {
    request: {
      url,
      useWhisper,
      model: model || null,
      masterPrompt: masterPrompt || null,
      cookieSource: cookieSource || null
    }
  }),
  openExternal: (url) => invoke('open_external', { url }),
  openFile: (filePath) => invoke('open_file', { filePath }),
  deleteSummary: (id) => invoke('delete_summary', {
    request: { id }
  }),
  sendSummaryToDiscord: (id, webhookUrl) => invoke('send_summary_to_discord', {
    request: {
      id,
      webhookUrl
    }
  }),
  translateSummary: (id, lang, model, promptTemplate) => invoke('translate_summary', {
    request: {
      id,
      lang,
      model: model || null,
      promptTemplate: promptTemplate || null
    }
  }),
  onSummarizeProgress: (callback) => listen('summarize-progress', (event) => {
    callback(String(event.payload || ''));
  }),
  onOpenSettings: (callback) => listen('open-settings', callback)
};

window.addEventListener('DOMContentLoaded', async () => {
  const form = document.getElementById('summarize-form');
  const urlInput = document.getElementById('url-input');
  const whisperCheckbox = document.getElementById('whisper-checkbox');
  const summariesContainer = document.getElementById('summaries-container');
  const loadingIndicator = document.getElementById('loading');
  const modelSelect = document.getElementById('model-select');
  const paginationTop = document.getElementById('pagination-top');
  const paginationBottom = document.getElementById('pagination-bottom');
  const summarizeButton = form.querySelector('button[type="submit"]');
  const autoTranslateCheckbox = document.getElementById('autotranslate-checkbox');
  const settingsDialog = document.getElementById('settings-dialog');
  const settingsPanel = settingsDialog.querySelector('.settings-panel');
  const settingsCloseButton = document.getElementById('settings-close-button');
  const masterPromptTextarea = document.getElementById('master-prompt-textarea');
  const resetMasterPromptButton = document.getElementById('reset-master-prompt-button');
  const discordWebhookInput = document.getElementById('discord-webhook-url-input');
  const youtubeCookieSourceSelect = document.getElementById('youtube-cookie-source-select');
  const youtubeCookieProfileInput = document.getElementById('youtube-cookie-profile-input');
  const youtubeCookieContainerInput = document.getElementById('youtube-cookie-container-input');
  const youtubeCookieKeyringSelect = document.getElementById('youtube-cookie-keyring-select');
  const youtubeCookieFileInput = document.getElementById('youtube-cookie-file-input');
  const clearYoutubeCookieSourceButton = document.getElementById('clear-youtube-cookie-source-button');
  const cookieDialog = document.getElementById('cookie-dialog');
  const cookiePanel = cookieDialog.querySelector('.cookie-panel');
  const cookieMessage = document.getElementById('cookie-message');
  const cookieSourceSelect = document.getElementById('cookie-source-select');
  const cookieProfileInput = document.getElementById('cookie-profile-input');
  const cookieContainerInput = document.getElementById('cookie-container-input');
  const cookieKeyringSelect = document.getElementById('cookie-keyring-select');
  const cookieFileInput = document.getElementById('cookie-file-input');
  const cookieUseButton = document.getElementById('cookie-use-button');
  const cookieCancelButton = document.getElementById('cookie-cancel-button');
  const translationPromptDeTextarea = document.getElementById('translation-prompt-de-textarea');
  const translationPromptJpTextarea = document.getElementById('translation-prompt-jp-textarea');
  const resetTranslationPromptDeButton = document.getElementById('reset-translation-prompt-de-button');
  const resetTranslationPromptJpButton = document.getElementById('reset-translation-prompt-jp-button');

  let fullSummaries = [];
  let currentPage = 1;
  const PAGE_SIZE = 20;
  let isLoading = false;
  let entryUiState = {};

  function setLoadingMessage(message) {
    if (!isLoading) {
      return;
    }
    loadingIndicator.style.display = 'inline';
    loadingIndicator.textContent = message;
  }

  function getMasterPrompt() {
    const savedPrompt = localStorage.getItem('masterPrompt');
    if (savedPrompt && savedPrompt.trim()) {
      return savedPrompt;
    }
    return DEFAULT_MASTER_PROMPT;
  }

  function getTranslationPrompt(lang) {
    const savedPrompt = localStorage.getItem(`translationPrompt.${lang}`);
    if (savedPrompt && savedPrompt.trim()) {
      return savedPrompt;
    }
    return DEFAULT_TRANSLATION_PROMPTS[lang];
  }

  function getDiscordWebhookUrl() {
    return (localStorage.getItem('discordWebhookUrl') || '').trim();
  }

  function loadYoutubeCookieSource() {
    const raw = localStorage.getItem(YOUTUBE_COOKIE_SOURCE_KEY);
    if (!raw) {
      return null;
    }
    try {
      const source = JSON.parse(raw);
      if (!source || typeof source !== 'object') {
        return null;
      }
      if (source.sourceType === 'browser' && source.browser) {
        return {
          sourceType: 'browser',
          browser: String(source.browser),
          profile: String(source.profile || '').trim() || null,
          keyring: String(source.keyring || '').trim() || null,
          container: String(source.container || '').trim() || null,
          cookiesFile: null
        };
      }
      if (source.sourceType === 'cookiesFile' && source.cookiesFile) {
        return {
          sourceType: 'cookiesFile',
          browser: null,
          profile: null,
          keyring: null,
          container: null,
          cookiesFile: String(source.cookiesFile).trim()
        };
      }
    } catch {
      localStorage.removeItem(YOUTUBE_COOKIE_SOURCE_KEY);
    }
    return null;
  }

  function saveYoutubeCookieSource(source) {
    if (!source) {
      localStorage.removeItem(YOUTUBE_COOKIE_SOURCE_KEY);
      return;
    }
    localStorage.setItem(YOUTUBE_COOKIE_SOURCE_KEY, JSON.stringify(source));
  }

  function isYoutubeCookieError(err) {
    const message = String(err?.message || err || '').toLowerCase();
    return YOUTUBE_COOKIE_ERROR_PATTERNS.some(pattern => message.includes(pattern));
  }

  function optionValueForSource(source) {
    if (!source) {
      return '';
    }
    if (source.sourceType === 'cookiesFile') {
      return 'cookiesFile';
    }
    if (source.sourceType === 'browser' && source.browser) {
      return `browser:${source.browser}`;
    }
    return '';
  }

  function sourceFromFields(select, profileInput, containerInput, keyringSelect, fileInput) {
    const value = select.value;
    if (!value) {
      return null;
    }
    if (value === 'cookiesFile') {
      const cookiesFile = fileInput.value.trim();
      if (!cookiesFile) {
        throw new Error('Enter a cookies.txt file path.');
      }
      return {
        sourceType: 'cookiesFile',
        browser: null,
        profile: null,
        keyring: null,
        container: null,
        cookiesFile
      };
    }
    if (value.startsWith('browser:')) {
      return {
        sourceType: 'browser',
        browser: value.slice('browser:'.length),
        profile: profileInput.value.trim() || null,
        keyring: keyringSelect.value || null,
        container: containerInput.value.trim() || null,
        cookiesFile: null
      };
    }
    return null;
  }

  function applySourceToFields(source, select, profileInput, containerInput, keyringSelect, fileInput) {
    select.value = optionValueForSource(source);
    profileInput.value = source?.profile || '';
    containerInput.value = source?.container || '';
    keyringSelect.value = source?.keyring || '';
    fileInput.value = source?.cookiesFile || '';
    updateCookieFieldVisibility(select, profileInput, containerInput, keyringSelect, fileInput);
  }

  function updateCookieFieldVisibility(select, profileInput, containerInput, keyringSelect, fileInput) {
    const isBrowser = select.value.startsWith('browser:');
    const isCookiesFile = select.value === 'cookiesFile';
    profileInput.closest('.settings-field-group').hidden = !isBrowser;
    containerInput.closest('.settings-field-group').hidden = !isBrowser;
    keyringSelect.closest('.settings-field-group').hidden = !isBrowser;
    fileInput.closest('.settings-field-group').hidden = !isCookiesFile;
  }

  function setCookieSelectOptions(select, sources, savedSource) {
    const currentValue = optionValueForSource(savedSource) || select.value;
    select.innerHTML = '';

    const noneOption = document.createElement('option');
    noneOption.value = '';
    noneOption.textContent = 'Ask when needed';
    select.appendChild(noneOption);

    (sources || []).forEach(source => {
      const option = document.createElement('option');
      option.value = `browser:${source.id}`;
      option.textContent = source.label;
      select.appendChild(option);
    });

    const cookiesFileOption = document.createElement('option');
    cookiesFileOption.value = 'cookiesFile';
    cookiesFileOption.textContent = 'cookies.txt file';
    select.appendChild(cookiesFileOption);

    if (currentValue && !Array.from(select.options).some(option => option.value === currentValue)) {
      const savedOption = document.createElement('option');
      savedOption.value = currentValue;
      savedOption.textContent = `${savedSource?.browser || 'Saved browser'} (not detected)`;
      select.insertBefore(savedOption, cookiesFileOption);
    }
    select.value = currentValue;
  }

  async function refreshYoutubeCookieSourceOptions() {
    let sources = [];
    try {
      sources = await window.api.getYoutubeCookieSources();
    } catch (err) {
      console.error('Failed to detect browsers for YouTube cookies:', err);
    }
    const savedSource = loadYoutubeCookieSource();
    setCookieSelectOptions(youtubeCookieSourceSelect, sources, savedSource);
    setCookieSelectOptions(cookieSourceSelect, sources, savedSource);
    applySourceToFields(
      savedSource,
      youtubeCookieSourceSelect,
      youtubeCookieProfileInput,
      youtubeCookieContainerInput,
      youtubeCookieKeyringSelect,
      youtubeCookieFileInput
    );
    applySourceToFields(
      savedSource,
      cookieSourceSelect,
      cookieProfileInput,
      cookieContainerInput,
      cookieKeyringSelect,
      cookieFileInput
    );
    return sources;
  }

  function saveSettingsCookieSource() {
    const source = sourceFromFields(
      youtubeCookieSourceSelect,
      youtubeCookieProfileInput,
      youtubeCookieContainerInput,
      youtubeCookieKeyringSelect,
      youtubeCookieFileInput
    );
    saveYoutubeCookieSource(source);
  }

  function openCookieDialog(message) {
    return new Promise(resolve => {
      cookieMessage.textContent = message;
      applySourceToFields(
        loadYoutubeCookieSource(),
        cookieSourceSelect,
        cookieProfileInput,
        cookieContainerInput,
        cookieKeyringSelect,
        cookieFileInput
      );
      cookieDialog.hidden = false;
      cookieSourceSelect.focus();

      const close = (result) => {
        cookieDialog.hidden = true;
        cookieUseButton.removeEventListener('click', useHandler);
        cookieCancelButton.removeEventListener('click', cancelHandler);
        cookieDialog.removeEventListener('click', dialogClickHandler);
        window.removeEventListener('keydown', keyHandler);
        resolve(result);
      };
      const useHandler = () => {
        try {
          const source = sourceFromFields(
            cookieSourceSelect,
            cookieProfileInput,
            cookieContainerInput,
            cookieKeyringSelect,
            cookieFileInput
          );
          if (!source) {
            alert('Select a browser or cookies.txt file.');
            return;
          }
          saveYoutubeCookieSource(source);
          applySourceToFields(
            source,
            youtubeCookieSourceSelect,
            youtubeCookieProfileInput,
            youtubeCookieContainerInput,
            youtubeCookieKeyringSelect,
            youtubeCookieFileInput
          );
          close(source);
        } catch (err) {
          alert(err.message);
        }
      };
      const cancelHandler = () => close(null);
      const dialogClickHandler = (event) => {
        if (!cookiePanel.contains(event.target)) {
          close(null);
        }
      };
      const keyHandler = (event) => {
        if (event.key === 'Escape' && !cookieDialog.hidden) {
          close(null);
        }
      };

      cookieUseButton.addEventListener('click', useHandler);
      cookieCancelButton.addEventListener('click', cancelHandler);
      cookieDialog.addEventListener('click', dialogClickHandler);
      window.addEventListener('keydown', keyHandler);
    });
  }

  async function summarizeWithCookieFlow(url, useWhisper, selectedModel, masterPrompt) {
    let cookieSource = loadYoutubeCookieSource();
    let authPromptMessage = 'YouTube requires browser cookies for this video. Select a signed-in browser and try again.';

    for (;;) {
      try {
        return await window.api.summarizeVideo(url, useWhisper, selectedModel, masterPrompt, cookieSource);
      } catch (err) {
        if (!isYoutubeCookieError(err)) {
          throw err;
        }
        if (cookieSource) {
          saveYoutubeCookieSource(null);
          authPromptMessage = 'The selected YouTube cookie source did not work. Make sure you are signed into YouTube, then choose a source again.';
          await refreshYoutubeCookieSourceOptions();
        }
        const selectedSource = await openCookieDialog(authPromptMessage);
        if (!selectedSource) {
          throw new Error('YouTube requires valid browser cookies for this video.');
        }
        cookieSource = selectedSource;
        authPromptMessage = 'The selected YouTube cookie source did not work. Choose another source or sign into YouTube in that browser.';
        setLoadingMessage('Retrying with YouTube cookies...');
      }
    }
  }

  function syncSettingsFields() {
    whisperCheckbox.checked = localStorage.getItem('useWhisper') === '0' ? false : true;
    autoTranslateCheckbox.checked = localStorage.getItem('autoTranslate') === '1' ? true : false;
    discordWebhookInput.value = getDiscordWebhookUrl();
    applySourceToFields(
      loadYoutubeCookieSource(),
      youtubeCookieSourceSelect,
      youtubeCookieProfileInput,
      youtubeCookieContainerInput,
      youtubeCookieKeyringSelect,
      youtubeCookieFileInput
    );
    masterPromptTextarea.value = getMasterPrompt();
    translationPromptDeTextarea.value = getTranslationPrompt('de');
    translationPromptJpTextarea.value = getTranslationPrompt('jp');
  }

  function openSettings() {
    refreshYoutubeCookieSourceOptions().finally(() => {
      syncSettingsFields();
      settingsDialog.hidden = false;
      discordWebhookInput.focus();
    });
  }

  function closeSettings() {
    settingsDialog.hidden = true;
  }

  await refreshYoutubeCookieSourceOptions();

  whisperCheckbox.checked = localStorage.getItem('useWhisper') === '0' ? false : true;
  autoTranslateCheckbox.checked = localStorage.getItem('autoTranslate') === '1' ? true : false;
  discordWebhookInput.value = getDiscordWebhookUrl();
  applySourceToFields(
    loadYoutubeCookieSource(),
    youtubeCookieSourceSelect,
    youtubeCookieProfileInput,
    youtubeCookieContainerInput,
    youtubeCookieKeyringSelect,
    youtubeCookieFileInput
  );
  masterPromptTextarea.value = getMasterPrompt();
  translationPromptDeTextarea.value = getTranslationPrompt('de');
  translationPromptJpTextarea.value = getTranslationPrompt('jp');

  whisperCheckbox.addEventListener('change', () => {
    localStorage.setItem('useWhisper', whisperCheckbox.checked ? '1' : '0');
  });
  autoTranslateCheckbox.addEventListener('change', () => {
    localStorage.setItem('autoTranslate', autoTranslateCheckbox.checked ? '1' : '0');
  });
  discordWebhookInput.addEventListener('input', () => {
    const webhookUrl = discordWebhookInput.value.trim();
    if (webhookUrl) {
      localStorage.setItem('discordWebhookUrl', webhookUrl);
    } else {
      localStorage.removeItem('discordWebhookUrl');
    }
  });
  youtubeCookieSourceSelect.addEventListener('change', () => {
    updateCookieFieldVisibility(
      youtubeCookieSourceSelect,
      youtubeCookieProfileInput,
      youtubeCookieContainerInput,
      youtubeCookieKeyringSelect,
      youtubeCookieFileInput
    );
    saveSettingsCookieSource();
  });
  youtubeCookieProfileInput.addEventListener('input', saveSettingsCookieSource);
  youtubeCookieContainerInput.addEventListener('input', saveSettingsCookieSource);
  youtubeCookieKeyringSelect.addEventListener('change', saveSettingsCookieSource);
  youtubeCookieFileInput.addEventListener('input', saveSettingsCookieSource);
  clearYoutubeCookieSourceButton.addEventListener('click', () => {
    saveYoutubeCookieSource(null);
    applySourceToFields(
      null,
      youtubeCookieSourceSelect,
      youtubeCookieProfileInput,
      youtubeCookieContainerInput,
      youtubeCookieKeyringSelect,
      youtubeCookieFileInput
    );
  });
  cookieSourceSelect.addEventListener('change', () => {
    updateCookieFieldVisibility(
      cookieSourceSelect,
      cookieProfileInput,
      cookieContainerInput,
      cookieKeyringSelect,
      cookieFileInput
    );
  });
  masterPromptTextarea.addEventListener('input', () => {
    localStorage.setItem('masterPrompt', masterPromptTextarea.value);
  });
  translationPromptDeTextarea.addEventListener('input', () => {
    localStorage.setItem('translationPrompt.de', translationPromptDeTextarea.value);
  });
  translationPromptJpTextarea.addEventListener('input', () => {
    localStorage.setItem('translationPrompt.jp', translationPromptJpTextarea.value);
  });
  resetMasterPromptButton.addEventListener('click', () => {
    masterPromptTextarea.value = DEFAULT_MASTER_PROMPT;
    localStorage.setItem('masterPrompt', DEFAULT_MASTER_PROMPT);
    masterPromptTextarea.focus();
  });
  resetTranslationPromptDeButton.addEventListener('click', () => {
    translationPromptDeTextarea.value = DEFAULT_TRANSLATION_PROMPTS.de;
    localStorage.setItem('translationPrompt.de', DEFAULT_TRANSLATION_PROMPTS.de);
    translationPromptDeTextarea.focus();
  });
  resetTranslationPromptJpButton.addEventListener('click', () => {
    translationPromptJpTextarea.value = DEFAULT_TRANSLATION_PROMPTS.jp;
    localStorage.setItem('translationPrompt.jp', DEFAULT_TRANSLATION_PROMPTS.jp);
    translationPromptJpTextarea.focus();
  });
  settingsCloseButton.addEventListener('click', closeSettings);
  settingsDialog.addEventListener('click', (event) => {
    if (!settingsPanel.contains(event.target)) {
      closeSettings();
    }
  });
  window.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && !settingsDialog.hidden) {
      closeSettings();
    }
  });

  function renderSummaries(list) {
    summariesContainer.innerHTML = '';
    const renderedIds = new Set();

    list.forEach(item => {
      renderedIds.add(item.id);
      if (!entryUiState[item.id]) {
        entryUiState[item.id] = { expanded: false, lang: 'en' };
      }
      let { expanded, lang } = entryUiState[item.id];

      const entry = document.createElement('div');
      entry.classList.add('entry');
      entry.style.overflow = 'hidden';

      const entryActions = document.createElement('div');
      entryActions.classList.add('entry-actions');

      const uploadButton = document.createElement('button');
      uploadButton.type = 'button';
      uploadButton.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 16V4"></path><path d="m7 9 5-5 5 5"></path><path d="M20 16v4H4v-4"></path></svg>';
      uploadButton.classList.add('entry-icon-button', 'upload-entry-button');
      uploadButton.title = 'Send to Discord';
      uploadButton.setAttribute('aria-label', 'Send to Discord');
      uploadButton.disabled = isLoading;
      uploadButton.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        if (isLoading) {
          return;
        }

        const webhookUrl = getDiscordWebhookUrl();
        if (!webhookUrl) {
          alert('Add a Discord Webhook URL in Settings first.');
          openSettings();
          return;
        }

        isLoading = true;
        summarizeButton.disabled = true;
        setLoadingMessage('Sending to Discord...');
        setActionLinksDisabled(true);

        window.api.sendSummaryToDiscord(item.id, webhookUrl)
          .catch(err => {
            alert('Error sending to Discord: ' + err.message);
          })
          .finally(() => {
            loadingIndicator.style.display = 'none';
            loadingIndicator.textContent = 'Loading...';
            summarizeButton.disabled = false;
            isLoading = false;
            setActionLinksDisabled(false);
          });
      });

      const deleteButton = document.createElement('button');
      deleteButton.type = 'button';
      deleteButton.innerHTML = '&times;';
      deleteButton.classList.add('entry-icon-button', 'delete-entry-button');
      deleteButton.title = 'Delete entry';
      deleteButton.setAttribute('aria-label', 'Delete entry');
      deleteButton.disabled = isLoading;
      deleteButton.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        if (isLoading) {
          return;
        }
        if (typeof confirmDialog !== 'function') {
          alert('Delete confirmation is unavailable.');
          return;
        }
        confirmDialog('Are you sure you want to delete this entry?', {
          title: 'Delete entry',
          kind: 'warning'
        }).then((confirmed) => {
          if (!confirmed) {
            return;
          }
          window.api.deleteSummary(item.id)
            .then(() => {
              delete entryUiState[item.id];
              return window.api.getSummaries().then(setSummaries);
            })
            .catch(err => {
              alert('Error deleting summary: ' + err.message);
            });
        });
      });
      const left = document.createElement('div');
      left.classList.add('left');
      if (item.thumbnail) {
        const img = document.createElement('img');
        img.src = toWebviewFileUrl(item.thumbnail);
        img.alt = item.video_name;
        img.classList.add('thumbnail');
        if (item.url) {
          img.style.cursor = 'pointer';
          img.title = 'Open video';
          img.addEventListener('click', (e) => {
            e.stopPropagation();
            window.api.openExternal(item.url);
          });
        }
        left.appendChild(img);
      }

      const langSwitcher = document.createElement('span');
      langSwitcher.style.display = 'flex';
      langSwitcher.style.gap = '6px';
      langSwitcher.style.marginTop = '8px';
      langSwitcher.style.marginBottom = '2px';

      const summaryFields = {
        en: item.summary_en,
        de: item.summary_de,
        jp: item.summary_jp
      };

      ['en', 'de', 'jp'].forEach(thisLang => {
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.textContent = thisLang.toUpperCase();
        btn.style.fontSize = '12px';
        btn.style.padding = '2px 8px';
        btn.style.borderRadius = '5px';
        btn.style.border = '1px solid #eee';
        btn.style.background = (thisLang === lang) ? '#9f1239' : '#fff1f2';
        btn.style.color = (thisLang === lang) ? '#fff' : '#9f1239';
        btn.disabled = isLoading;
        btn.addEventListener('click', () => {
          lang = thisLang;
          entryUiState[item.id].lang = lang;
          renderSummaryContent();
          Array.from(langSwitcher.children).forEach((button, index) => {
            const language = ['en', 'de', 'jp'][index];
            button.style.background = (language === lang) ? '#9f1239' : '#fff1f2';
            button.style.color = (language === lang) ? '#fff' : '#9f1239';
          });
        });
        langSwitcher.appendChild(btn);
      });
      left.appendChild(langSwitcher);

      const middle = document.createElement('div');
      middle.classList.add('middle');
      const headline = document.createElement('div');
      headline.style.display = 'flex';
      headline.style.alignItems = 'center';
      headline.style.justifyContent = 'space-between';
      headline.style.gap = '12px';
      const headlineMain = document.createElement('div');
      headlineMain.style.display = 'flex';
      headlineMain.style.alignItems = 'center';
      headlineMain.style.minWidth = '0';
      const titleEl = document.createElement('strong');
      titleEl.style.display = 'block';
      titleEl.style.fontSize = '16px';
      titleEl.style.cursor = 'default';
      titleEl.style.marginLeft = '0';
      titleEl.textContent = item.video_name;

      const arrow = document.createElement('span');
      arrow.textContent = expanded ? '▼' : '▶';
      arrow.style.marginRight = '8px';
      arrow.style.marginLeft = '0';
      arrow.style.fontSize = '18px';
      arrow.style.userSelect = 'none';
      arrow.style.transition = 'transform 0.15s';

      headlineMain.appendChild(arrow);
      headlineMain.appendChild(titleEl);
      headline.appendChild(headlineMain);
      entryActions.appendChild(uploadButton);
      entryActions.appendChild(deleteButton);
      headline.appendChild(entryActions);

      const channelEl = document.createElement('span');
      channelEl.style.fontSize = '14px';
      channelEl.style.opacity = '0.8';
      channelEl.style.marginBottom = '12px';
      channelEl.textContent = item.channel || '';
      channelEl.style.display = 'block';
      channelEl.style.marginTop = '2px';

      middle.appendChild(headline);
      middle.appendChild(channelEl);

      const summaryHTML = document.createElement('div');
      summaryHTML.classList.add('summary');
      summaryHTML.style.display = '-webkit-box';
      summaryHTML.style.webkitBoxOrient = 'vertical';
      summaryHTML.style.overflow = 'hidden';
      summaryHTML.style.transition = 'max-height 0.2s';

      function renderSummaryContent() {
        const text = summaryFields[lang];
        summaryHTML.innerHTML = '';
        if (text && text.trim()) {
          summaryHTML.innerHTML = markdownToHTML(text);
        } else {
          const missingMsg = document.createElement('span');
          missingMsg.textContent = (
            lang === 'de' ? 'German not available. ' :
            lang === 'jp' ? 'Japanese not available. ' :
            'Not available. '
          );
          summaryHTML.appendChild(missingMsg);
        }
        if (!expanded) {
          summaryHTML.style.display = '-webkit-box';
          summaryHTML.style.webkitLineClamp = '2';
          summaryHTML.style.maxHeight = '2.8em';
        } else {
          summaryHTML.style.display = 'block';
          summaryHTML.style.webkitLineClamp = '';
          summaryHTML.style.maxHeight = '';
        }
      }

      summaryHTML.addEventListener('click', (event) => {
        const target = event.target instanceof Element ? event.target : null;
        const link = target?.closest('a[href]');
        if (!link || !summaryHTML.contains(link)) {
          return;
        }
        const href = link.getAttribute('href');
        if (!href || href.startsWith('#')) {
          return;
        }
        event.preventDefault();
        window.api.openExternal(href).catch(err => {
          console.error('Failed to open summary link:', err);
        });
      });

      middle.appendChild(summaryHTML);

      entry.appendChild(left);
      entry.appendChild(middle);

      summariesContainer.appendChild(entry);

      function applyCollapsedStyle() {
        if (!expanded) {
          entry.classList.add('collapsed');
          arrow.textContent = '▶';
        } else {
          entry.classList.remove('collapsed');
          arrow.textContent = '▼';
        }
        renderSummaryContent();
      }
      applyCollapsedStyle();

      middle.addEventListener('click', () => {
        if (!expanded) {
          expanded = true;
          entryUiState[item.id].expanded = true;
          applyCollapsedStyle();
        }
      });

      headline.addEventListener('click', (e) => {
        if (expanded) {
          expanded = false;
          entryUiState[item.id].expanded = false;
          applyCollapsedStyle();
          e.stopPropagation();
        }
      });
    });

    Object.keys(entryUiState).forEach(id => {
      if (!renderedIds.has(Number(id))) {
        delete entryUiState[id];
      }
    });

    setActionLinksDisabled(isLoading);
  }

  function setActionLinksDisabled(disabled) {
    document.querySelectorAll('.entry-icon-button').forEach(button => {
      if (disabled) {
        button.disabled = true;
        button.style.opacity = '0.5';
      } else {
        button.disabled = false;
        button.style.opacity = '';
      }
    });
    document.querySelectorAll('.left button').forEach(btn => {
      btn.disabled = disabled;
      btn.style.opacity = disabled ? '0.5' : '';
    });
  }

  function updatePaginationControls() {
    if (!fullSummaries || fullSummaries.length <= PAGE_SIZE) {
      paginationTop.style.display = 'none';
      paginationBottom.style.display = 'none';
      return;
    }
    paginationTop.style.display = 'flex';
    paginationBottom.style.display = 'flex';
    const totalPages = Math.ceil(fullSummaries.length / PAGE_SIZE);

    const buildNav = (container) => {
      container.innerHTML = '';

      const prevBtn = document.createElement('button');
      prevBtn.textContent = '«';
      prevBtn.disabled = currentPage === 1;
      prevBtn.addEventListener('click', () => {
        if (currentPage > 1) {
          showPage(currentPage - 1);
          updatePaginationControls();
        }
      });
      container.appendChild(prevBtn);

      for (let i = 1; i <= totalPages; i += 1) {
        const btn = document.createElement('button');
        btn.textContent = i;
        if (i === currentPage) {
          btn.classList.add('active');
        }
        btn.addEventListener('click', () => {
          showPage(i);
          updatePaginationControls();
        });
        container.appendChild(btn);
      }

      const nextBtn = document.createElement('button');
      nextBtn.textContent = '»';
      nextBtn.disabled = currentPage === totalPages;
      nextBtn.addEventListener('click', () => {
        if (currentPage < totalPages) {
          showPage(currentPage + 1);
          updatePaginationControls();
        }
      });
      container.appendChild(nextBtn);
    };

    buildNav(paginationTop);
    buildNav(paginationBottom);
  }

  function showPage(page) {
    const totalPages = Math.ceil(fullSummaries.length / PAGE_SIZE);
    currentPage = Math.max(1, Math.min(page, totalPages || 1));
    const start = (currentPage - 1) * PAGE_SIZE;
    const end = start + PAGE_SIZE;
    renderSummaries(fullSummaries.slice(start, end));
  }

  function setSummaries(list) {
    fullSummaries = list || [];
    const totalPages = Math.ceil(fullSummaries.length / PAGE_SIZE);
    if (currentPage > totalPages) {
      currentPage = Math.max(1, totalPages);
    }
    showPage(currentPage);
    updatePaginationControls();
  }

  try {
    const models = await window.api.getModels();
    modelSelect.innerHTML = '';
    const hasMistral = Array.isArray(models) && models.includes('mistral:latest');
    const placeholder = document.createElement('option');
    placeholder.disabled = true;
    placeholder.value = '';
    placeholder.innerText = 'Select model';
    modelSelect.appendChild(placeholder);
    if (Array.isArray(models)) {
      models.forEach(name => {
        const option = document.createElement('option');
        option.value = name;
        option.innerText = name;
        modelSelect.appendChild(option);
      });
    }
    const saved = localStorage.getItem('selectedModel');
    let toSelect = '';
    if (saved && models.includes(saved)) {
      toSelect = saved;
    } else if (hasMistral) {
      toSelect = 'mistral:latest';
    }
    if (toSelect) {
      modelSelect.value = toSelect;
      placeholder.selected = false;
    } else {
      placeholder.selected = true;
    }
  } catch (err) {
    console.error('Error loading models:', err);
    modelSelect.innerHTML = '';
    const placeholder = document.createElement('option');
    placeholder.disabled = true;
    placeholder.selected = true;
    placeholder.value = '';
    placeholder.innerText = 'Select model';
    modelSelect.appendChild(placeholder);
  }

  modelSelect.addEventListener('change', () => {
    localStorage.setItem('selectedModel', modelSelect.value);
  });

  window.api.getSummaries().then(setSummaries).catch(console.error);

  form.addEventListener('submit', (e) => {
    e.preventDefault();
    const url = urlInput.value.trim();
    const useWhisper = whisperCheckbox.checked;
    const autoTranslate = autoTranslateCheckbox.checked;
    if (!url || isLoading) {
      return;
    }

    isLoading = true;
    summarizeButton.disabled = true;
    setLoadingMessage('Summarizing…');
    setActionLinksDisabled(true);

    const selectedModel = modelSelect.value;
    summarizeWithCookieFlow(url, useWhisper, selectedModel, getMasterPrompt())
      .then((newEntry) => {
        if (!newEntry || !newEntry.id) {
          return window.api.getSummaries().then(setSummaries);
        }

        entryUiState[newEntry.id] = { expanded: true, lang: 'en' };

        if (!autoTranslate) {
          return window.api.getSummaries().then(setSummaries);
        }

        let translationsOk = true;
        setLoadingMessage('Translating to German (DE)…');
        return window.api.translateSummary(newEntry.id, 'de', selectedModel, getTranslationPrompt('de'))
          .then(() => {
            setLoadingMessage('Translating to Japanese (JP)…');
            return window.api.translateSummary(newEntry.id, 'jp', selectedModel, getTranslationPrompt('jp'));
          })
          .catch(err => {
            translationsOk = false;
            alert('Error translating summary: ' + err.message);
          })
          .then(() => {
            entryUiState[newEntry.id] = {
              expanded: true,
              lang: translationsOk ? 'jp' : 'en'
            };
            return window.api.getSummaries().then(setSummaries);
          });
      })
      .catch(err => {
        alert('Error summarizing video: ' + err.message);
      })
      .finally(() => {
        loadingIndicator.style.display = 'none';
        loadingIndicator.textContent = 'Loading…';
        summarizeButton.disabled = false;
        isLoading = false;
        setActionLinksDisabled(false);
        urlInput.value = '';
      });
  });

  window.api.onSummarizeProgress(line => {
    if (!isLoading || !line) {
      return;
    }
    setLoadingMessage(line);
  });
  window.api.onOpenSettings(openSettings);
});
