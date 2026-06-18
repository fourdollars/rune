# Popover Emoji Picker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the note custom icon text input field with a popover emoji picker consisting of categorized emojis, category tabs, and search filtering.

**Architecture:** Update HTML structure in `web/index.html` to hold the picker trigger and overlay popover, add CSS layouts and styling rules in `web/style.css`, and implement rendering, categories scroll navigation, event toggles, click-outside dismissal, and search matching in `web/app.js`.

**Tech Stack:** HTML, CSS, JavaScript.

## Global Constraints
- Clean compiling via `cargo check`.
- Perfect formatting via `cargo fmt --all`.
- Non-broken static asset unit tests.

---

### Task 1: UI Layout Update

**Files:**
- Modify: `web/index.html`

- [ ] **Step 1: Replace input field with popover UI structure in `web/index.html`**
  Find the "Icon (Emoji)" form group inside `note-settings-modal` (around line 98) and replace it with:
  ```html
              <div class="form-group emoji-picker-group">
                  <label>Icon (Emoji)</label>
                  <div class="emoji-picker-wrapper">
                      <button type="button" id="emoji-picker-trigger" class="emoji-picker-trigger">📂</button>
                      <div id="emoji-picker-popover" class="emoji-picker-popover hidden">
                          <input type="text" id="emoji-search-input" class="emoji-search-input" placeholder="Search emoji..." autocomplete="off" />
                          <div class="emoji-tabs" id="emoji-picker-tabs"></div>
                          <div class="emoji-scroll-area" id="emoji-picker-scroll-area">
                              <button type="button" class="emoji-clear-btn" id="emoji-clear-btn">Reset to Default</button>
                              <div id="emoji-categories-container"></div>
                          </div>
                      </div>
                  </div>
              </div>
  ```

- [ ] **Step 2: Run static asset check tests**
  Run: `cargo test serve::static_files::tests::test_static_assets_present`
  Expected: PASS

- [ ] **Step 3: Commit changes**
  Run:
  ```bash
  git add web/index.html
  git commit -m "feat(web): update note settings dialog layout to support popover emoji picker"
  ```

---

### Task 2: Picker Styling Update

**Files:**
- Modify: `web/style.css`

- [ ] **Step 1: Add emoji picker styles to the end of `web/style.css`**
  Append the following CSS rules:
  ```css
  /* Emoji Picker Styles */
  .emoji-picker-wrapper {
      position: relative;
      display: inline-block;
  }
  .emoji-picker-trigger {
      font-size: 24px;
      padding: 8px 12px;
      border: 1px solid var(--border);
      background: var(--bg-secondary);
      border-radius: 6px;
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      transition: border-color 0.2s;
  }
  .emoji-picker-trigger:hover {
      border-color: var(--accent);
  }
  .emoji-picker-popover {
      position: absolute;
      top: calc(100% + 6px);
      left: 0;
      z-index: 1000;
      background: var(--bg-primary);
      border: 1px solid var(--border);
      border-radius: 6px;
      box-shadow: 0 4px 16px rgba(0,0,0,0.25);
      padding: 10px;
      width: 280px;
      box-sizing: border-box;
  }
  .emoji-search-input {
      width: 100%;
      padding: 6px 10px;
      border: 1px solid var(--border);
      background: var(--bg-secondary);
      color: var(--text-primary);
      border-radius: 4px;
      font-size: 13px;
      margin-bottom: 8px;
      box-sizing: border-box;
  }
  .emoji-tabs {
      display: flex;
      justify-content: space-between;
      border-bottom: 1px solid var(--border);
      padding-bottom: 6px;
      margin-bottom: 8px;
  }
  .emoji-tab-btn {
      background: none;
      border: none;
      font-size: 16px;
      cursor: pointer;
      padding: 4px;
      border-radius: 4px;
      transition: background 0.15s;
  }
  .emoji-tab-btn:hover {
      background: var(--bg-hover);
  }
  .emoji-scroll-area {
      max-height: 220px;
      overflow-y: auto;
      padding-right: 4px;
  }
  .emoji-clear-btn {
      width: 100%;
      padding: 6px;
      margin-bottom: 10px;
      border: 1px solid var(--border);
      background: var(--bg-secondary);
      color: var(--text-primary);
      border-radius: 4px;
      font-size: 13px;
      cursor: pointer;
  }
  .emoji-clear-btn:hover {
      background: var(--bg-hover);
      border-color: var(--accent);
  }
  .emoji-category-section {
      margin-bottom: 12px;
  }
  .emoji-category-title {
      font-size: 11px;
      font-weight: bold;
      color: var(--text-secondary);
      margin-bottom: 6px;
      text-transform: uppercase;
      letter-spacing: 0.5px;
  }
  .emoji-grid {
      display: grid;
      grid-template-columns: repeat(7, 1fr);
      gap: 4px;
  }
  .emoji-btn {
      font-size: 18px;
      padding: 4px;
      border: 1px solid transparent;
      background: none;
      border-radius: 4px;
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      transition: background 0.15s, border-color 0.15s;
  }
  .emoji-btn:hover {
      background: var(--bg-hover);
      border-color: var(--border);
  }
  .emoji-btn.active {
      border-color: var(--accent);
      background: var(--bg-hover);
  }
  ```

- [ ] **Step 2: Verify static files unit tests**
  Run: `cargo test serve::static_files::tests::test_style_has_split_view`
  Expected: PASS

- [ ] **Step 3: Commit changes**
  Run:
  ```bash
  git add web/style.css
  git commit -m "feat(web): add styling rules for popover emoji picker"
  ```

---

### Task 3: Emoji Picker Logic Implementation

**Files:**
- Modify: `web/app.js`

- [ ] **Step 1: Define the structured, categorized emoji list & initialize listeners**
  Create the structured EMOJI_CATEGORIES list with tags inside `web/app.js` (e.g. inside `initApp()` or alongside other global config variables):
  ```javascript
  const EMOJI_CATEGORIES = {
      'smileys': {
          icon: '😀',
          title: 'Smileys',
          list: [
              { char: '😀', tags: 'smiley smile happy grin face' },
              { char: '😃', tags: 'smiley smile happy grin face' },
              { char: '😄', tags: 'smiley smile happy grin face' },
              { char: '😁', tags: 'smiley smile happy grin face' },
              { char: '😆', tags: 'smiley smile happy grin face' },
              { char: '😅', tags: 'smiley smile happy grin sweat face' },
              { char: '🤣', tags: 'smiley laugh rofl face' },
              { char: '😂', tags: 'smiley laugh tear face' },
              { char: '🙂', tags: 'smiley smile face' },
              { char: '🙃', tags: 'smiley upside down face' },
              { char: '😉', tags: 'smiley wink face' },
              { char: '😊', tags: 'smiley smile blush face' },
              { char: '😇', tags: 'smiley angel halo face' },
              { char: '🥰', tags: 'smiley love hearts blush face' },
              { char: '😍', tags: 'smiley love hearts eyes face' },
              { char: '🤩', tags: 'smiley star eyes face' },
              { char: '😘', tags: 'smiley love kiss face' },
              { char: '😋', tags: 'smiley yum delicious face' },
              { char: '😛', tags: 'smiley tongue face' },
              { char: '😜', tags: 'smiley tongue wink face' },
              { char: '🤪', tags: 'smiley crazy tongue face' },
              { char: '🤑', tags: 'smiley money mouth face' },
              { char: '😎', tags: 'smiley cool sunglasses face' },
              { char: '🤓', tags: 'smiley nerd glasses face' },
              { char: '🧐', tags: 'smiley monocle face' },
              { char: '🤔', tags: 'smiley think face' },
              { char: '😐', tags: 'smiley neutral face' },
              { char: '😑', tags: 'smiley expressionless face' },
              { char: '😏', tags: 'smiley smirk face' },
              { char: '😒', tags: 'smiley unamused face' },
              { char: '😬', tags: 'smiley grimace face' },
              { char: '🤥', tags: 'smiley lie liar face' },
              { char: '😌', tags: 'smiley relieved face' },
              { char: '😔', tags: 'smiley pensive face' },
              { char: '😪', tags: 'smiley sleepy tear face' },
              { char: '😴', tags: 'smiley sleep face' },
              { char: '😷', tags: 'smiley mask sick face' },
              { char: '🤢', tags: 'smiley nauseous green face' },
              { char: '🤮', tags: 'smiley vomit face' },
              { char: '🥵', tags: 'smiley hot red sweat face' },
              { char: '🥶', tags: 'smiley cold blue ice face' },
              { char: '😵', tags: 'smiley dizzy face' },
              { char: '🤯', tags: 'smiley mind blown explode head face' },
              { char: '🥳', tags: 'smiley party celebrate face' },
              { char: '💀', tags: 'skull dead bones' },
              { char: '💩', tags: 'poop dump' },
              { char: '🔥', tags: 'fire hot lit burn' },
              { char: '✨', tags: 'sparkles gold shine magic' },
              { char: '🌟', tags: 'star gold glow' },
              { char: '⭐', tags: 'star gold' },
              { char: '❤️', tags: 'love heart red' },
              { char: '💖', tags: 'love heart sparkles' },
              { char: '💔', tags: 'heart broken' }
          ]
      },
      'animals': {
          icon: '🐶',
          title: 'Nature',
          list: [
              { char: '🐶', tags: 'dog puppy animal pet' },
              { char: '🐱', tags: 'cat kitty animal pet' },
              { char: '🐭', tags: 'mouse animal' },
              { char: '🐰', tags: 'rabbit bunny animal' },
              { char: '🦊', tags: 'fox animal' },
              { char: '🐻', tags: 'bear animal' },
              { char: '🐼', tags: 'panda animal' },
              { char: '🐨', tags: 'koala animal' },
              { char: '🐯', tags: 'tiger animal' },
              { char: '🦁', tags: 'lion animal' },
              { char: '🐮', tags: 'cow animal' },
              { char: '🐷', tags: 'pig animal' },
              { char: '🐸', tags: 'frog animal' },
              { char: '🐵', tags: 'monkey animal' },
              { char: '🐒', tags: 'monkey animal' },
              { char: '🐧', tags: 'penguin animal' },
              { char: '🐦', tags: 'bird animal' },
              { char: '🦆', tags: 'duck animal' },
              { char: '🦅', tags: 'eagle animal' },
              { char: '🦉', tags: 'owl animal' },
              { char: '🐺', tags: 'wolf animal' },
              { char: '🦄', tags: 'unicorn animal magic' },
              { char: '🐝', tags: 'bee insect' },
              { char: '🐛', tags: 'bug caterpillar insect' },
              { char: '🦋', tags: 'butterfly insect' },
              { char: '🕷️', tags: 'spider insect' },
              { char: '🐢', tags: 'turtle animal' },
              { char: '🐍', tags: 'snake animal' },
              { char: '🐙', tags: 'octopus ocean sea' },
              { char: '🐬', tags: 'dolphin ocean sea' },
              { char: '🐳', tags: 'whale ocean sea' },
              { char: '🦈', tags: 'shark ocean sea' },
              { char: '🌲', tags: 'tree forest pine green' },
              { char: '🌵', tags: 'cactus desert green' },
              { char: '🍀', tags: 'clover leaf green luck' },
              { char: '🍁', tags: 'maple leaf red fall' },
              { char: '🌸', tags: 'flower blossom pink spring' },
              { char: '🌹', tags: 'rose flower red love' },
              { char: '🌻', tags: 'sunflower flower yellow' }
          ]
      },
      'food': {
          icon: '🍔',
          title: 'Food & Drink',
          list: [
              { char: '🍏', tags: 'apple green fruit food' },
              { char: '🍎', tags: 'apple red fruit food' },
              { char: '🍊', tags: 'orange fruit food' },
              { char: '🍌', tags: 'banana fruit food' },
              { char: '🍉', tags: 'watermelon fruit food' },
              { char: '🍇', tags: 'grape fruit food' },
              { char: '🍓', tags: 'strawberry fruit food' },
              { char: '🍒', tags: 'cherry fruit food' },
              { char: '🍑', tags: 'peach fruit food' },
              { char: '🍍', tags: 'pineapple fruit food' },
              { char: '🥥', tags: 'coconut fruit food' },
              { char: '🥝', tags: 'kiwi fruit food' },
              { char: '🍅', tags: 'tomato vegetable food' },
              { char: '🍆', tags: 'eggplant aubergine vegetable food' },
              { char: '🥑', tags: 'avocado vegetable food' },
              { char: '🌽', tags: 'corn vegetable food' },
              { char: '🥕', tags: 'carrot vegetable food' },
              { char: '🥔', tags: 'potato vegetable food' },
              { char: '🥐', tags: 'croissant bread bakery food' },
              { char: '🍞', tags: 'bread toast bakery food' },
              { char: '🥓', tags: 'bacon meat food' },
              { char: '🥩', tags: 'steak meat food' },
              { char: '🍔', tags: 'hamburger burger fastfood food' },
              { char: '🍕', tags: 'pizza fastfood food' },
              { char: '🌭', tags: 'hotdog fastfood food' },
              { char: '🍟', tags: 'fries fastfood food' },
              { char: '🥚', tags: 'egg food' },
              { char: '🍿', tags: 'popcorn movie snack food' },
              { char: '🥟', tags: 'dumpling dimsum food' },
              { char: '🍣', tags: 'sushi japanese food' },
              { char: '🍷', tags: 'wine glass drink alcohol' },
              { char: '🍺', tags: 'beer mug drink alcohol' },
              { char: '🍻', tags: 'beers cheers drink alcohol' },
              { char: '☕', tags: 'coffee cafe mug drink hot' },
              { char: '🍵', tags: 'tea green cup drink hot' }
          ]
      },
      'activities': {
          icon: '⚽',
          title: 'Activities',
          list: [
              { char: '⚽', tags: 'soccer football ball sports' },
              { char: '🏀', tags: 'basketball ball sports' },
              { char: '🏈', tags: 'football ball sports' },
              { char: '⚾', tags: 'baseball ball sports' },
              { char: '🥎', tags: 'softball ball sports' },
              { char: '🎾', tags: 'tennis ball sports' },
              { char: '🏐', tags: 'volleyball ball sports' },
              { char: '🎱', tags: 'billiards pool ball sports' },
              { char: '🏓', tags: 'pingpong table tennis ball sports' },
              { char: '🏸', tags: 'badminton sports' },
              { char: '🏹', tags: 'archery bow arrow sports' },
              { char: 'Fishing', tags: '🎣' },
              { char: '🎣', tags: 'fishing rod fish sports' },
              { char: '🎯', tags: 'dart target bullseye' },
              { char: '🪁', tags: 'kite fly' },
              { char: '🎮', tags: 'controller game video sports console' },
              { char: '🕹️', tags: 'joystick game video console' },
              { char: '🎰', tags: 'slot machine game casino' },
              { char: '🎲', tags: 'die dice game board' },
              { char: '🧩', tags: 'puzzle piece game' },
              { char: '🎨', tags: 'paint palette art design' },
              { char: '🎬', tags: 'clapperboard movie cinema' },
              { char: '🎤', tags: 'microphone singing music' },
              { char: '🎧', tags: 'headphones music audio' },
              { char: '🎹', tags: 'piano keyboard music instrument' },
              { char: '🥁', tags: 'drum music instrument' },
              { char: '🎸', tags: 'guitar music instrument' },
              { char: '🎻', tags: 'violin music instrument' }
          ]
      },
      'travel': {
          icon: '🚗',
          title: 'Travel',
          list: [
              { char: '🚗', tags: 'car red automobile travel transport' },
              { char: '🚕', tags: 'taxi cab yellow travel transport' },
              { char: '🚓', tags: 'police car travel transport' },
              { char: '🚒', tags: 'fire engine truck travel transport' },
              { char: '🚐', tags: 'van travel transport' },
              { char: '🚚', tags: 'truck transport' },
              { char: '🚜', tags: 'tractor transport farm' },
              { char: '🛵', tags: 'scooter transport' },
              { char: '🏍️', tags: 'motorcycle bike transport' },
              { char: '🚨', tags: 'police siren emergency light' },
              { char: '🚲', tags: 'bicycle bike transport sports' },
              { char: '⛽', tags: 'gas fuel station' },
              { char: '⚓', tags: 'anchor ship boat navy' },
              { char: '⛵', tags: 'sailboat ship boat travel' },
              { char: '🛶', tags: 'canoe kayak boat travel' },
              { char: '✈️', tags: 'airplane plane flight travel' },
              { char: '🚀', tags: 'rocket space launch work speed' },
              { char: '🚁', tags: 'helicopter travel flight' },
              { char: '🏕️', tags: 'camping outdoor travel' },
              { char: '🏠', tags: 'house home building' },
              { char: '🏡', tags: 'house garden home building' },
              { char: '🏢', tags: 'office business building' },
              { char: '🏥', tags: 'hospital medical building' },
              { char: '🏫', tags: 'school education building' },
              { char: '🏭', tags: 'factory building industry' },
              { char: '🏰', tags: 'castle fortress building' },
              { char: '⛩️', tags: 'shrine torii gate Japanese' },
              { char: '⛲', tags: 'fountain park' },
              { char: '🌌', tags: 'milkyway galaxy space night' },
              { char: '🌉', tags: 'bridge night' }
          ]
      },
      'objects': {
          icon: '💡',
          title: 'Objects',
          list: [
              { char: '⌚', tags: 'watch time clock' },
              { char: '📱', tags: 'phone mobile smartphone cell' },
              { char: '💻', tags: 'laptop computer tech' },
              { char: '⌨️', tags: 'keyboard tech computer' },
              { char: '🖥️', tags: 'monitor screen computer' },
              { char: '🖨️', tags: 'printer office paper' },
              { char: '📸', tags: 'camera photo picture' },
              { char: '🎥', tags: 'camera movie video' },
              { char: '☎️', tags: 'telephone phone landline' },
              { char: '📺', tags: 'tv television screen' },
              { char: '📻', tags: 'radio music audio' },
              { char: '🎙️', tags: 'microphone studio recording' },
              { char: '🧭', tags: 'compass direction travel' },
              { char: '⏰', tags: 'alarm clock time' },
              { char: '⏳', tags: 'hourglass time sand' },
              { char: '🔋', tags: 'battery power energy charge' },
              { char: '💡', tags: 'lightbulb idea light bulb' },
              { char: '🔦', tags: 'flashlight torch light' },
              { char: '🕯️', tags: 'candle light wax' },
              { char: '🧯', tags: 'extinguisher safety fire' },
              { char: '💵', tags: 'dollar cash money green' },
              { char: '🪙', tags: 'coin gold money cash' },
              { char: '💰', tags: 'money bag gold cash' },
              { char: '💳', tags: 'credit card money bank' },
              { char: '💎', tags: 'gem diamond jewel' },
              { char: '⚖️', tags: 'scales justice balance' },
              { char: '🔨', tags: 'hammer tool construction' },
              { char: '🔧', tags: 'wrench spanner tool fix' },
              { char: '🔩', tags: 'bolt nut screw tool hardware' },
              { char: '⚙️', tags: 'gear settings tool mechanics' },
              { char: '🔐', tags: 'lock key secure private' },
              { char: '🔒', tags: 'lock secure private' },
              { char: '🔓', tags: 'lock open insecure' },
              { char: '🔑', tags: 'key lock open' },
              { char: '📚', tags: 'books study read education' },
              { char: '📝', tags: 'pencil paper write memo note' },
              { char: '📌', tags: 'pushpin pin map post' },
              { char: '✉️', tags: 'envelope mail letter' },
              { char: '🔔', tags: 'bell notification alert' }
          ]
      },
      'flags': {
          icon: '🚩',
          title: 'Flags',
          list: [
              { char: '🏁', tags: 'flag checkered finish race' },
              { char: '🚩', tags: 'flag red post' },
              { char: '🎌', tags: 'flags crossed Japan festival' },
              { char: '🏴', tags: 'flag black' },
              { char: '🏳️', tags: 'flag white peace surrender' },
              { char: '🏳️‍🌈', tags: 'flag rainbow pride lgbt' },
              { char: '🏳️‍⚧️', tags: 'flag transgender pride lgbt' },
              { char: '🏴‍☠️', tags: 'flag pirate skull crossbones' }
          ]
      }
  };
  ```

- [ ] **Step 2: Initialize picker layout structure in `web/app.js` on startup**
  Write a helper `initEmojiPicker()` and call it during initialization:
  ```javascript
  let selectedNoteIcon = null;

  function initEmojiPicker() {
      const tabsContainer = document.getElementById('emoji-picker-tabs');
      const categoriesContainer = document.getElementById('emoji-categories-container');
      if (!tabsContainer || !categoriesContainer) return;

      tabsContainer.innerHTML = '';
      categoriesContainer.innerHTML = '';

      Object.keys(EMOJI_CATEGORIES).forEach(key => {
          const category = EMOJI_CATEGORIES[key];
          
          // Render Tab button
          const tabBtn = document.createElement('button');
          tabBtn.type = 'button';
          tabBtn.className = 'emoji-tab-btn';
          tabBtn.textContent = category.icon;
          tabBtn.title = category.title;
          tabBtn.onclick = (e) => {
              e.stopPropagation();
              const targetHeader = document.getElementById(`category-sec-${key}`);
              if (targetHeader) {
                  targetHeader.scrollIntoView({ behavior: 'smooth', block: 'start' });
              }
          };
          tabsContainer.appendChild(tabBtn);

          // Render Category Section
          const section = document.createElement('div');
          section.className = 'emoji-category-section';
          section.id = `category-sec-${key}`;

          const title = document.createElement('div');
          title.className = 'emoji-category-title';
          title.textContent = category.title;
          section.appendChild(title);

          const grid = document.createElement('div');
          grid.className = 'emoji-grid';

          category.list.forEach(emoji => {
              const btn = document.createElement('button');
              btn.type = 'button';
              btn.className = 'emoji-btn';
              btn.textContent = emoji.char;
              btn.title = emoji.tags;
              btn.onclick = (e) => {
                  e.stopPropagation();
                  selectEmoji(emoji.char);
              };
              grid.appendChild(btn);
          });

          section.appendChild(grid);
          categoriesContainer.appendChild(section);
      });

      // Search Event Listener
      const searchInput = document.getElementById('emoji-search-input');
      if (searchInput) {
          searchInput.oninput = () => filterEmojis(searchInput.value.trim().toLowerCase());
      }

      // Reset Button Listener
      const clearBtn = document.getElementById('emoji-clear-btn');
      if (clearBtn) {
          clearBtn.onclick = (e) => {
              e.stopPropagation();
              selectEmoji(null);
          };
      }

      // Popover click behavior
      const trigger = document.getElementById('emoji-picker-trigger');
      const popover = document.getElementById('emoji-picker-popover');
      if (trigger && popover) {
          trigger.onclick = (e) => {
              e.stopPropagation();
              popover.classList.toggle('hidden');
              if (!popover.classList.contains('hidden') && searchInput) {
                  searchInput.value = '';
                  filterEmojis('');
                  searchInput.focus();
              }
          };
      }

      // Click outside dismissal
      document.addEventListener('click', (e) => {
          if (popover && !popover.classList.contains('hidden')) {
              const wrapper = document.querySelector('.emoji-picker-wrapper');
              if (wrapper && !wrapper.contains(e.target)) {
                  popover.classList.add('hidden');
              }
          }
      });
  }

  function selectEmoji(emoji) {
      selectedNoteIcon = emoji;
      const trigger = document.getElementById('emoji-picker-trigger');
      if (trigger) trigger.textContent = emoji || '📂';
      
      // Update active state class in popover grid
      document.querySelectorAll('.emoji-btn').forEach(btn => {
          if (btn.textContent === emoji) {
              btn.classList.add('active');
          } else {
              btn.classList.remove('active');
          }
      });

      const popover = document.getElementById('emoji-picker-popover');
      if (popover) popover.classList.add('hidden');
  }

  function filterEmojis(query) {
      const container = document.getElementById('emoji-categories-container');
      const tabs = document.getElementById('emoji-picker-tabs');
      if (!container) return;

      if (!query) {
          // Show tabs and categories normally
          if (tabs) tabs.style.display = 'flex';
          initEmojiPicker(); // Re-render standard category state
          selectEmoji(selectedNoteIcon);
          return;
      }

      // Hide category tabs during search
      if (tabs) tabs.style.display = 'none';
      container.innerHTML = '';

      // Render flat grid of search results
      const resultsSection = document.createElement('div');
      resultsSection.className = 'emoji-category-section';

      const title = document.createElement('div');
      title.className = 'emoji-category-title';
      title.textContent = 'Search Results';
      resultsSection.appendChild(title);

      const grid = document.createElement('div');
      grid.className = 'emoji-grid';

      let count = 0;
      Object.keys(EMOJI_CATEGORIES).forEach(key => {
          const category = EMOJI_CATEGORIES[key];
          category.list.forEach(emoji => {
              if (emoji.tags.toLowerCase().includes(query)) {
                  count++;
                  const btn = document.createElement('button');
                  btn.type = 'button';
                  btn.className = 'emoji-btn';
                  if (emoji.char === selectedNoteIcon) btn.classList.add('active');
                  btn.textContent = emoji.char;
                  btn.title = emoji.tags;
                  btn.onclick = (e) => {
                      e.stopPropagation();
                      selectEmoji(emoji.char);
                  };
                  grid.appendChild(btn);
              }
          });
      });

      if (count === 0) {
          const noResults = document.createElement('div');
          noResults.style.fontSize = '13px';
          noResults.style.color = 'var(--text-secondary)';
          noResults.style.padding = '10px 0';
          noResults.textContent = 'No matching emojis';
          grid.appendChild(noResults);
      }

      resultsSection.appendChild(grid);
      container.appendChild(resultsSection);
  }
  ```

- [ ] **Step 3: Modify Note Settings populate / save functions**
  Populate picker trigger in `showNoteSettings(sessionId)`:
  ```javascript
      selectedNoteIcon = s.icon || null;
      const trigger = document.getElementById('emoji-picker-trigger');
      if (trigger) trigger.textContent = selectedNoteIcon || '📂';
      // Highlight currently selected button:
      document.querySelectorAll('.emoji-btn').forEach(btn => {
          if (selectedNoteIcon && btn.textContent === selectedNoteIcon) {
              btn.classList.add('active');
          } else {
              btn.classList.remove('active');
          }
      });
  ```
  And read picker state in `saveNoteSettings()`:
  ```javascript
  function saveNoteSettings() {
      if (!settingsNoteId) return;
      const name = document.getElementById('note-settings-name').value.trim();
      const s = notes.find(x => x.id === settingsNoteId);
      if (s && name) {
          if (name !== s.name || selectedNoteIcon !== (s.icon || null)) {
              api('note/rename', { note_id: settingsNoteId, name, icon: selectedNoteIcon });
          }
      }
      hideNoteSettings();
  }
  ```

- [ ] **Step 4: Initialize picker on app startup**
  Inside `initApp()` or inside DOM load callback, add `initEmojiPicker();`.

- [ ] **Step 5: Run tests and formatting check**
  Run: `cargo test`
  Expected: PASS
  Run: `cargo fmt --all -- --check`
  Expected: PASS

- [ ] **Step 6: Commit changes**
  Run:
  ```bash
  git add web/app.js
  git commit -m "feat(web): implement popover emoji picker logic, navigation, and search filter"
  ```
