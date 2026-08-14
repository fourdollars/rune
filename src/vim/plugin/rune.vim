" rune.vim - Native Zero-Trust AI Assistant Plugin for Vim 9+
" Version: 0.1.0

if exists('g:loaded_rune')
  finish
endif
let g:loaded_rune = 1

let s:rune_job = v:null
let s:buf_versions = {}
let s:log_history = []

function! s:Log(msg) abort
  let time_str = strftime('%H:%M:%S')
  let entry = '[' . time_str . '] ' . a:msg
  call add(s:log_history, entry)
  if len(s:log_history) > 300
    call remove(s:log_history, 0)
  endif
  call s:UpdateLogBuffer()
endfunction

function! s:UpdateLogBuffer() abort
  let winnr = bufwinnr('__Rune_Log__')
  if winnr > 0
    let curr_win = winnr()
    execute winnr . 'wincmd w'
    setlocal modifiable
    silent %delete _
    call setline(1, s:log_history)
    normal! G
    setlocal nomodifiable
    execute curr_win . 'wincmd w'
  endif
endfunction

function! s:StartRuneServer() abort
  if s:rune_job isnot v:null
    if has('nvim') && s:rune_job > 0
      return
    elseif !has('nvim') && job_status(s:rune_job) ==# 'run'
      return
    endif
  endif

  let rune_bin = get(g:, 'rune_binary_path', 'rune')
  let args = [rune_bin, 'vim', 'stdio']
  call s:Log('Starting daemon: ' . join(args, ' '))

  if has('nvim')
    let s:rune_job = jobstart(args, {
          \ 'on_stdout': function('s:OnStdioMessage'),
          \ 'on_stderr': function('s:OnStdioErr'),
          \ 'on_exit': function('s:OnStdioExit'),
          \ 'stdout_buffered': v:false,
          \ })
  else
    let s:rune_job = job_start(args, {
          \ 'out_mode': 'nl',
          \ 'out_cb': function('s:OnVimMessage'),
          \ 'err_cb': function('s:OnVimErr'),
          \ 'exit_cb': function('s:OnVimExit'),
          \ })
  endif

  call s:SendRpc('rune/initialize', {'protocol_version': 1})
  call s:SendRpc('rune/status', {})
endfunction

function! s:OnVimMessage(channel, msg) abort
  call s:ProcessJsonMessage(a:msg)
endfunction

function! s:OnVimErr(channel, msg) abort
  call s:Log('[STDERR] ' . a:msg)
endfunction

function! s:OnVimExit(job, status) abort
  call s:Log('[EXIT] Code: ' . a:status)
endfunction

function! s:OnStdioMessage(job_id, data, event) abort
  for line in a:data
    if !empty(line)
      call s:ProcessJsonMessage(line)
    endif
  endfor
endfunction

function! s:OnStdioErr(job_id, data, event) abort
  for line in a:data
    if !empty(line)
      call s:Log('[STDERR] ' . line)
    endif
  endfor
endfunction

function! s:OnStdioExit(job_id, data, event) abort
  call s:Log('[EXIT] Code: ' . a:data)
endfunction

let s:rune_status = 'idle'
let s:rune_model = ''
let s:rune_provider = ''
let s:rune_thinking = ''

function! rune#statusline() abort
  if !get(g:, 'rune_enabled', 1) || !get(b:, 'rune_enabled', 1)
    return 'ᚱᚢᚾᛖ [OFF]'
  endif
  let st = s:rune_status
  let model_str = get(s:, 'rune_model', '')
  let think_str = get(s:, 'rune_thinking', '')
  let detail = !empty(model_str) ? (model_str . (!empty(think_str) ? ' ' . think_str : '')) : ''
  let bracket_info = !empty(detail) ? ' (' . detail . ')' : ''

  if st ==# 'working'
    return 'ᚱᚢᚾᛖ [working]' . bracket_info
  elseif st ==# 'error'
    return 'ᚱᚢᚾᛖ [error]'
  elseif st ==# 'rate_limited'
    return 'ᚱᚢᚾᛖ [limited]'
  else
    return 'ᚱᚢᚾᛖ' . bracket_info
  endif
endfunction

function! rune#status_full() abort
  if !get(g:, 'rune_enabled', 1) || !get(b:, 'rune_enabled', 1)
    return 'ᚱᚢᚾᛖ [OFF]'
  endif
  let st = s:rune_status
  let model_info = !empty(s:rune_model) ? s:rune_model : 'active'
  let think_info = !empty(s:rune_thinking) ? ' (thinking: ' . s:rune_thinking . ')' : ''

  if st ==# 'working'
    return 'ᚱᚢᚾᛖ [working...] [' . model_info . ']' . think_info
  elseif st ==# 'error'
    return 'ᚱᚢᚾᛖ [error] [' . model_info . ']'
  elseif st ==# 'rate_limited'
    return 'ᚱᚢᚾᛖ [limited] [' . model_info . ']'
  else
    return 'ᚱᚢᚾᛖ [' . model_info . ']' . think_info
  endif
endfunction

function! rune#model() abort
  return s:rune_model
endfunction

function! rune#thinking() abort
  return s:rune_thinking
endfunction

function! rune#provider() abort
  return s:rune_provider
endfunction

let s:available_models = []
let s:available_thinking_levels = ['off', 'low', 'medium', 'high', 'xhigh']

function! rune#CompleteModels(ArgLead, CmdLine, CursorPos) abort
  return filter(copy(s:available_models), 'v:val =~? "^" . a:ArgLead')
endfunction

function! rune#CompleteThinking(ArgLead, CmdLine, CursorPos) abort
  return filter(copy(s:available_thinking_levels), 'v:val =~? "^" . a:ArgLead')
endfunction

function! s:ProcessJsonMessage(raw_msg) abort
  call s:Log('[IN] ' . a:raw_msg)
  try
    let msg = json_decode(a:raw_msg)
    if has_key(msg, 'result') && !empty(msg.result)
      let res = msg.result
      if has_key(res, 'active_model') && !empty(res.active_model)
        let s:rune_model = res.active_model
        echom "ᚱᚢᚾᛖ Active model: " . res.active_model
      endif
      if has_key(res, 'active_thinking')
        let s:rune_thinking = (res.active_thinking ==# 'off' ? '' : res.active_thinking)
        echom "ᚱᚢᚾᛖ Thinking level: " . (empty(s:rune_thinking) ? 'off' : s:rune_thinking)
      endif
      if has_key(res, 'available_models')
        let s:available_models = res.available_models
      endif
      if has_key(res, 'available_levels')
        let s:available_thinking_levels = res.available_levels
      endif
      if has_key(res, 'model') && !empty(res.model)
        let s:rune_model = res.model
      endif
      if has_key(res, 'provider') && !empty(res.provider)
        let s:rune_provider = res.provider
      endif
      if has_key(res, 'thinking')
        let s:rune_thinking = (res.thinking ==# 'off' ? '' : res.thinking)
      endif
      if has_key(res, 'state')
        let s:rune_status = res.state
      endif
      redrawstatus
      if has_key(res, 'candidates') && !empty(res.candidates)
        let cur_buf = bufnr('%')
        if !has_key(res, 'buffer_id') || !has_key(res, 'version') || (res.buffer_id == cur_buf && res.version == get(s:buf_versions, cur_buf, 0))
          call s:RenderGhostText(res.candidates[0].text)
        endif
      elseif has_key(res, 'text')
        call s:DisplayChatResult(res.text)
      elseif has_key(res, 'edits')
        call s:ApplyEdits(res.edits)
      endif
    elseif has_key(msg, 'error') && !empty(msg.error)
      let s:rune_status = 'error'
      redrawstatus
      echohl ErrorMsg
      echom "ᚱᚢᚾᛖ Error: " . msg.error.message
      echohl None
    endif
  catch
  endtry
endfunction

function! s:DisplayChatResult(text) abort
  let cur_win = winnr()
  let winnr = bufwinnr('__Rune_Chat__')
  let width = get(g:, 'rune_chat_width', 80)
  if winnr > 0
    execute winnr . 'wincmd w'
    execute 'vertical resize ' . width
  else
    execute 'vertical botright ' . width . 'new __Rune_Chat__'
    setlocal buftype=nofile bufhidden=wipe noswapfile filetype=markdown wrap
    nnoremap <buffer> q :close<CR>
  endif
  setlocal modifiable
  silent %delete _
  call setline(1, split(a:text, "\n"))
  setlocal nomodifiable
  execute cur_win . 'wincmd w'
  echom "ᚱᚢᚾᛖ Answer received."
endfunction

function! s:ApplyEdits(edits) abort
  for edit in a:edits
    let start_l = edit.start_line
    let end_l = edit.end_line
    let new_lines = split(edit.new_text, "\n")
    if start_l <= line('$')
      execute start_l . ',' . end_l . 'delete _'
      call append(start_l - 1, new_lines)
    endif
  endfor
  echom "ᚱᚢᚾᛖ Edit applied."
endfunction

let s:active_ghost_text = ''

function! s:RenderGhostText(text) abort
  call s:ClearGhostText()
  if empty(a:text) || mode() !~# '^[iR]'
    return
  endif

  let s:active_ghost_text = a:text

  if has('nvim')
    let ns = nvim_create_namespace('rune_ghost')
    call nvim_buf_set_extmark(0, ns, line('.') - 1, col('.') - 1, {
          \ 'virt_text': [[a:text, 'Comment']],
          \ 'virt_text_pos': 'inline'
          \ })
  elseif exists('*prop_add')
    if empty(prop_type_get('RuneGhostText'))
      call prop_type_add('RuneGhostText', {'highlight': 'Comment'})
    endif
    let c = col('.')
    let line_len = strlen(getline('.'))
    if c > line_len
      call prop_add(line('.'), 0, {'type': 'RuneGhostText', 'text': a:text, 'text_align': 'after'})
    else
      call prop_add(line('.'), c, {'type': 'RuneGhostText', 'text': a:text})
    endif
  endif
endfunction

function! s:ClearGhostText() abort
  let s:active_ghost_text = ''
  if has('nvim')
    let ns = nvim_create_namespace('rune_ghost')
    call nvim_buf_clear_namespace(0, ns, 0, -1)
  elseif exists('*prop_remove')
    if !empty(prop_type_get('RuneGhostText'))
      call prop_remove({'type': 'RuneGhostText', 'all': v:true})
    endif
  endif
endfunction

function! rune#Accept() abort
  if !empty(s:active_ghost_text)
    let text = s:active_ghost_text
    call s:ClearGhostText()
    return text
  endif
  return "\<Tab>"
endfunction

function! rune#AcceptWord() abort
  if !empty(s:active_ghost_text)
    let m = matchstr(s:active_ghost_text, '^\s*\S\+')
    if !empty(m)
      let s:active_ghost_text = s:active_ghost_text[len(m):]
      return m
    endif
  endif
  return ''
endfunction

function! rune#AcceptLine() abort
  if !empty(s:active_ghost_text)
    let lines = split(s:active_ghost_text, "\n")
    if !empty(lines)
      let line_text = lines[0]
      let s:active_ghost_text = join(lines[1:], "\n")
      return line_text
    endif
  endif
  return ''
endfunction

function! rune#Dismiss() abort
  call s:ClearGhostText()
  return ''
endfunction

function! s:OnTextChanged() abort
  if !get(g:, 'rune_enabled', 1) || !get(b:, 'rune_enabled', 1)
    return
  endif

  if mode() !~# '^[iR]'
    return
  endif

  let l:line = line('.')
  let l:col = col('.')
  let l:lines = getline(1, '$')
  let l:cur_line = getline('.')

  let l:prefix = join(l:lines[0 : l:line - 2], "\n")
  if !empty(l:prefix)
    let l:prefix .= "\n"
  endif
  let l:cur_prefix = l:cur_line[0 : l:col - 2]
  let l:prefix .= l:cur_prefix

  let l:suffix = l:cur_line[l:col - 1 :]
  if l:line < len(l:lines)
    let l:suffix .= "\n" . join(l:lines[l:line :], "\n")
  endif

  " Only trigger completion on non-alphanumeric characters (e.g. !, ., space, :, (, etc.) unless trigger_on_word is enabled
  let l:trigger_pattern = get(g:, 'rune_trigger_pattern', '[^a-zA-Z0-9_]')
  if !empty(l:trigger_pattern) && !empty(l:cur_prefix)
    let l:last_char = l:cur_prefix[-1:]
    if l:last_char !~# l:trigger_pattern
      call s:ClearGhostText()
      return
    endif
  endif

  let l:buf = bufnr('%')
  let s:buf_versions[l:buf] = get(s:buf_versions, l:buf, 0) + 1

  call s:SendRpc('rune/completion', {
        \ 'buffer_id': l:buf,
        \ 'version': s:buf_versions[l:buf],
        \ 'filepath': expand('%:p'),
        \ 'language': &filetype,
        \ 'prefix': l:prefix,
        \ 'suffix': l:suffix,
        \ 'line': l:line - 1,
        \ 'character': l:col - 1,
        \ })
endfunction

augroup RunePlugin
  autocmd!
  autocmd VimEnter * call s:StartRuneServer()
  autocmd InsertLeave,CursorMovedI * call s:ClearGhostText()
  autocmd TextChangedI * call s:OnTextChanged()
augroup END

function! s:SendRpc(method, params) abort
  call s:StartRuneServer()
  let req = {'jsonrpc': '2.0', 'id': get(s:, 'rpc_seq', 1), 'method': a:method, 'params': a:params}
  let s:rpc_seq = get(s:, 'rpc_seq', 1) + 1
  let raw = json_encode(req)
  call s:Log('[OUT] ' . raw)
  if has('nvim')
    call chansend(s:rune_job, raw . "\n")
  else
    call ch_sendraw(job_getchannel(s:rune_job), raw . "\n")
  endif
endfunction

function! s:GetSelection(line1, line2) abort
  let mode = mode()
  if mode =~# "[vV\<C-v>]"
    let [_, l1, c1, _] = getpos("'<")
    let [_, l2, c2, _] = getpos("'>")
    let lines = getline(l1, l2)
    if empty(lines)
      return {}
    endif
    if mode ==# 'v'
      if l1 == l2
        let lines[0] = lines[0][c1 - 1 : c2 - 1]
      else
        let lines[0] = lines[0][c1 - 1 :]
        let lines[-1] = lines[-1][: c2 - 1]
      endif
    endif
    return {
          \ 'start_line': l1,
          \ 'end_line': l2,
          \ 'text': join(lines, "\n")
          \ }
  elseif a:line1 > 0 && a:line2 > 0 && (a:line1 != 1 || a:line2 != line('$'))
    let lines = getline(a:line1, a:line2)
    return {
          \ 'start_line': a:line1,
          \ 'end_line': a:line2,
          \ 'text': join(lines, "\n")
          \ }
  endif
  return {}
endfunction

function! s:RuneAsk(line1, line2, prompt) abort
  if empty(a:prompt)
    echom "Usage: :RuneAsk <question>"
    return
  endif
  let params = {
        \ 'buffer_id': bufnr('%'),
        \ 'prompt': a:prompt,
        \ 'filepath': expand('%:p'),
        \ 'language': &filetype,
        \ }
  let sel = s:GetSelection(a:line1, a:line2)
  if !empty(sel)
    let params.selection = sel
  endif
  echom "ᚱᚢᚾᛖ Asking AI..."
  call s:SendRpc('rune/chat', params)
endfunction

function! s:RuneExplain(line1, line2, prompt) abort
  let p = !empty(a:prompt) ? a:prompt : 'Explain the purpose and logic of this file/selection in detail.'
  let params = {
        \ 'buffer_id': bufnr('%'),
        \ 'prompt': p,
        \ 'filepath': expand('%:p'),
        \ 'language': &filetype,
        \ }
  let sel = s:GetSelection(a:line1, a:line2)
  if !empty(sel)
    let params.selection = sel
  endif
  echom "ᚱᚢᚾᛖ Explaining code..."
  call s:SendRpc('rune/chat', params)
endfunction

function! s:RuneEdit(line1, line2, prompt) abort
  if empty(a:prompt)
    echom "Usage: :RuneEdit <instructions>"
    return
  endif
  let params = {
        \ 'buffer_id': bufnr('%'),
        \ 'prompt': a:prompt,
        \ 'filepath': expand('%:p'),
        \ 'language': &filetype,
        \ }
  let sel = s:GetSelection(a:line1, a:line2)
  if !empty(sel)
    let params.selection = sel
  endif
  echom "ᚱᚢᚾᛖ Editing code..."
  call s:SendRpc('rune/edit', params)
endfunction

function! s:RuneFix(line1, line2, prompt) abort
  let p = !empty(a:prompt) ? a:prompt : 'Fix any bugs, potential errors or performance issues in this file.'
  call s:RuneEdit(a:line1, a:line2, p)
endfunction

function! s:RuneToggle(bang) abort
  if a:bang ==# '!'
    let g:rune_enabled = !get(g:, 'rune_enabled', 1)
    echom "Rune global status: " . (g:rune_enabled ? "Enabled" : "Disabled")
  else
    let b:rune_enabled = !get(b:, 'rune_enabled', 1)
    echom "Rune buffer status: " . (b:rune_enabled ? "Enabled" : "Disabled")
  endif
endfunction

function! s:RuneStatus() abort
  call s:SendRpc('rune/status', {})
endfunction

function! s:RuneLog() abort
  let winnr = bufwinnr('__Rune_Log__')
  if winnr > 0
    execute winnr . 'wincmd w'
  else
    let width = get(g:, 'rune_log_width', 45)
    execute 'vertical botright ' . width . 'new __Rune_Log__'
    setlocal buftype=nofile bufhidden=wipe noswapfile filetype=log
    nnoremap <buffer> q :close<CR>
  endif
  call s:UpdateLogBuffer()
endfunction

function! s:RuneModel(model) abort
  if empty(a:model)
    call s:SendRpc('rune/model', {})
  else
    call s:SendRpc('rune/model', {'model': a:model})
  endif
endfunction

function! s:RuneThink(level) abort
  if empty(a:level)
    call s:SendRpc('rune/thinking', {})
  else
    call s:SendRpc('rune/thinking', {'thinking': a:level})
  endif
endfunction

command! -range=% -nargs=* RuneAsk call s:RuneAsk(<line1>, <line2>, <q-args>)
command! -range=% -nargs=* RuneExplain call s:RuneExplain(<line1>, <line2>, <q-args>)
command! -range=% -nargs=* RuneEdit call s:RuneEdit(<line1>, <line2>, <q-args>)
command! -range=% -nargs=* RuneFix call s:RuneFix(<line1>, <line2>, <q-args>)
command! -nargs=? -complete=customlist,rune#CompleteModels RuneModel call s:RuneModel(<q-args>)
command! -nargs=? -complete=customlist,rune#CompleteThinking RuneThink call s:RuneThink(<q-args>)
command! -nargs=0 -bang RuneToggle call s:RuneToggle("<bang>")
command! -nargs=0 RuneStatus call s:RuneStatus()
command! -nargs=0 RuneLog call s:RuneLog()

if get(g:, 'rune_no_map_tab', 0) == 0
  inoremap <expr> <Tab> rune#Accept()
  inoremap <expr> <C-g>w rune#AcceptWord()
  inoremap <expr> <C-g>l rune#AcceptLine()
  inoremap <expr> <C-g>d rune#Dismiss()
endif
