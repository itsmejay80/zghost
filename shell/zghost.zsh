# zghost: inline autosuggestions for Zsh, backed by a Rust daemon.
# Load with:  eval "$(zghost init zsh)"
#
# Typing streams the buffer to the daemon over a Unix socket; replies arrive
# asynchronously through `zle -F` and are drawn as ghost text in POSTDISPLAY.

[[ -o interactive ]] || return 0
zmodload zsh/net/socket zsh/zselect zsh/datetime zsh/zutil 2>/dev/null || return 0
autoload -Uz add-zle-hook-widget add-zsh-hook

# Re-sourcing .zshrc: drop the previous connection before resetting state.
(( $+functions[_zghost_disconnect] )) && _zghost_disconnect

typeset -g ZGHOST_BIN=${ZGHOST_BIN:-__ZGHOST_BIN__}
typeset -g ZGHOST_SOCKET=${ZGHOST_SOCKET:-${${TMPDIR:-/tmp}%/}/zghost-$UID.sock}
typeset -g ZGHOST_HIGHLIGHT=${ZGHOST_HIGHLIGHT:-fg=8}

typeset -g _zghost_fd= _zghost_highlight= _zghost_last_buffer= _zghost_cmd= _zghost_cmd_dir=
typeset -g _zghost_tab_fallback=expand-or-complete
typeset -gi _zghost_req=0 _zghost_index=1 _zghost_retry_at=0
typeset -ga _zghost_candidates=() _zghost_candidate_cursors=()
typeset -gi _zghost_menu=0 _zghost_menu_native=0 _zghost_menu_cursor=0 _zghost_word_start=0 _zghost_word_end=0
typeset -g _zghost_menu_buffer= _zghost_keymap=emacs _zghost_request_kind=S


# Escape \, newline and tab; the daemon's protocol is one line per message.
_zghost_encode() { REPLY=${${${1//\\/\\\\}//$'\n'/\\n}//$'\t'/\\t} }

_zghost_disconnect() {
  [[ -n $_zghost_fd ]] || return 0
  zle -F $_zghost_fd 2>/dev/null
  exec {_zghost_fd}>&-
  _zghost_fd=
}

_zghost_connect() {
  [[ -n $_zghost_fd ]] && return 0
  (( EPOCHSECONDS < _zghost_retry_at )) && return 1
  local i
  for i in {1..50}; do
    if zsocket $ZGHOST_SOCKET 2>/dev/null; then
      _zghost_fd=$REPLY
      zle -F -w $_zghost_fd _zghost-response 2>/dev/null
      return 0
    fi
    (( i == 1 )) && ( $ZGHOST_BIN daemon --socket $ZGHOST_SOCKET \
        --histfile ${HISTFILE:-$HOME/.zsh_history} </dev/null &>/dev/null & )
    zselect -t 2
  done
  _zghost_retry_at=$(( EPOCHSECONDS + 3 ))
  return 1
}

_zghost_send() {
  _zghost_connect || return 1
  print -rnu $_zghost_fd -- "$1"$'\n' 2>/dev/null || { _zghost_disconnect; return 1 }
}

_zghost_request() {
  local cwd
  _zghost_request_kind=S
  _zghost_encode $PWD; cwd=$REPLY
  _zghost_encode $BUFFER
  _zghost_send "S"$'\t'$(( ++_zghost_req ))$'\t'$$$'\t'$cwd$'\t'$REPLY
}

# zsh shifts region_highlight offsets as text is inserted, so our entry can't
# be found by its exact text later. On zsh >= 5.9 tag it with memo= instead.
autoload -Uz is-at-least
if is-at-least 5.9; then
  typeset -g _zghost_memo=",memo=zghost"
else
  typeset -g _zghost_memo=
fi

_zghost_clear_highlight() {
  if [[ -n $_zghost_memo ]]; then
    region_highlight=("${(@)region_highlight:#*memo=zghost*}")
  elif [[ -n $_zghost_highlight ]]; then
    region_highlight=("${(@)region_highlight:#${(b)_zghost_highlight}}")
  fi
  _zghost_highlight=
}

_zghost_render() {
  _zghost_clear_highlight
  local s=$_zghost_candidates[_zghost_index]
  if (( CURSOR == $#BUFFER )) && [[ -n $s && $s == "$BUFFER"?* ]]; then
    POSTDISPLAY=${s:$#BUFFER}
    _zghost_highlight="$#BUFFER $(( $#BUFFER + $#POSTDISPLAY )) $ZGHOST_HIGHLIGHT$_zghost_memo"
    region_highlight+=("$_zghost_highlight")
  else
    POSTDISPLAY=
  fi
}

# Called by `zle -F` when the daemon replies: "<id>\t<candidate>...".
_zghost-response() {
  local line
  if [[ -n $2 ]] || ! IFS= read -r -u $1 line; then
    _zghost_disconnect
    if (( _zghost_menu )); then
      local native=$_zghost_menu_native
      zghost-menu-dismiss
      (( native )) && zle $_zghost_tab_fallback
    fi
    return
  fi
  local -a fields=("${(@ps:\t:)line}")
  [[ $fields[1] == $_zghost_req ]] || return 0
  local f decoded
  _zghost_candidates=()
  _zghost_candidate_cursors=()
  if [[ $_zghost_request_kind == C ]]; then
    (( _zghost_menu )) || return 0
    [[ $BUFFER == $_zghost_menu_buffer && $CURSOR == $_zghost_menu_cursor ]] || { zghost-menu-dismiss; return 0 }
    local span=${fields[2]#@}
    _zghost_word_start=${span%%:*}
    _zghost_word_end=${span#*:}
    for f in "${(@)fields[3,-1]}"; do
      decoded=${(g::)f}
      _zghost_candidate_cursors+=("${decoded%%:*}")
      _zghost_candidates+=("${decoded#*:}")
    done
    # Native completion functions run only on explicit invocation, never per key.
    if (( $#_zghost_candidates == 0 && _zghost_menu_native )) && (( $+functions[_main_complete] )); then
      zle zghost-native-collect
    fi
    if (( $#_zghost_candidates == 0 )); then
      local native=$_zghost_menu_native
      zghost-menu-dismiss
      (( native )) && zle $_zghost_tab_fallback
      return 0
    fi
    _zghost_menu=2
    _zghost_index=1
    _zghost_menu_draw
  else
    for f in "${(@)fields[2,-1]}"; do
      _zghost_candidates+=("${(g::)f}")
    done
    _zghost_index=1
    _zghost_render
  fi
  zle -R
}

# Ask about any text, and about an empty prompt (next-command prediction)
# unless ZGHOST_NO_PREDICT is set.
_zghost_wants() {
  [[ -n ${BUFFER//[[:space:]]/} ]] || [[ -z $BUFFER && -z $ZGHOST_NO_PREDICT ]]
}

_zghost_pre_redraw() {
  if (( _zghost_menu )) && [[ $BUFFER != $_zghost_menu_buffer || $CURSOR != $_zghost_menu_cursor ]]; then
    zghost-menu-dismiss
  fi
  if [[ $BUFFER != "$_zghost_last_buffer" ]]; then
    _zghost_last_buffer=$BUFFER
    # Keep candidates that still fit so ghost text doesn't flicker while typing.
    _zghost_candidates=("${(@M)_zghost_candidates:#${(b)BUFFER}?*}")
    _zghost_index=1
    if _zghost_wants && (( PENDING == 0 )); then
      if [[ -n $ZGHOST_MENU_AUTO && -n $BUFFER ]]; then
        _zghost_open_menu 0
      else
        _zghost_request
      fi
    fi
  fi
  (( _zghost_menu == 2 )) && _zghost_menu_draw || _zghost_render
}

# A fresh prompt never changes the (empty) buffer, so ask here. precmd has
# already reported the last command, so the daemon knows what came before.
_zghost_line_init() {
  _zghost_last_buffer=$BUFFER
  _zghost_candidates=()
  _zghost_wants && _zghost_request
}

_zghost_line_finish() {
  (( _zghost_menu )) && zghost-menu-dismiss
  _zghost_clear_highlight
  POSTDISPLAY=
  _zghost_last_buffer=
  _zghost_candidates=()
}

_zghost_accept_or() {
  # POSTDISPLAY also contains transient menu rows while the menu is open.
  (( _zghost_menu )) && { zghost-menu-accept; return 0 }
  if [[ -n $POSTDISPLAY ]] && (( CURSOR == $#BUFFER )); then
    BUFFER+=$POSTDISPLAY
    CURSOR=$#BUFFER
  else
    zle $1
  fi
}

zghost-accept() { _zghost_accept_or forward-char }
zghost-accept-end() { _zghost_accept_or end-of-line }

# With ZGHOST_TAB_ACCEPT=1, Tab accepts ghost text; otherwise it completes.
zghost-tab() {
  if [[ -n $ZGHOST_TAB_ACCEPT && -n $POSTDISPLAY ]]; then
    zghost-accept
  else
    _zghost_open_menu 1
  fi
}

zghost-accept-word() {
  (( _zghost_menu )) && { zghost-menu-accept; return 0 }
  setopt localoptions extendedglob
  if [[ -n $POSTDISPLAY ]] && (( CURSOR == $#BUFFER )); then
    BUFFER+=${(M)POSTDISPLAY##[[:space:]]#[^[:space:]]##}
    CURSOR=$#BUFFER
  else
    zle forward-word
  fi
}

_zghost_cycle() {
  if (( _zghost_menu )); then
    (( $1 > 0 )) && zghost-menu-down || zghost-menu-up
    return 0
  fi
  local n=$#_zghost_candidates
  if [[ -n $POSTDISPLAY ]] && (( n > 1 )); then
    _zghost_index=$(( (_zghost_index - 1 + $1 + n) % n + 1 ))
    _zghost_render
  else
    zle $2
  fi
}

zghost-next() { _zghost_cycle 1 history-search-forward }
zghost-prev() { _zghost_cycle -1 history-search-backward }

# Explicit completions use a separate request: unlike ghost text, these can
# replace a word (including in the middle of a command).
_zghost_open_menu() {
  (( _zghost_menu )) && { zghost-menu-down; return 0 }
  _zghost_menu=1
  _zghost_menu_native=$1
  _zghost_menu_buffer=$BUFFER
  _zghost_menu_cursor=$CURSOR
  _zghost_keymap=$KEYMAP
  _zghost_candidates=()
  _zghost_candidate_cursors=()
  _zghost_clear_highlight
  POSTDISPLAY=
  zle -K zghost-menu
  local cwd
  _zghost_request_kind=C
  _zghost_encode $PWD; cwd=$REPLY
  _zghost_encode $BUFFER
  _zghost_send "C"$'\t'$(( ++_zghost_req ))$'\t'$$$'\t'$cwd$'\t'$CURSOR$'\t'$REPLY || {
    local native=$_zghost_menu_native
    zghost-menu-dismiss
    (( native )) && zle $_zghost_tab_fallback
  }
}
zghost-complete() { _zghost_open_menu 1 }

_zghost_menu_draw() {
  local -a lines=()
  local i marker begin=$(( ((_zghost_index - 1) / 8) * 8 + 1 ))
  local end=$(( begin + 7 ))
  (( end > $#_zghost_candidates )) && end=$#_zghost_candidates
  for (( i=begin; i<=end; i++ )); do
    # (V) makes control characters visible instead of emitting terminal escapes.
    marker=" "
    (( i == _zghost_index )) && marker=">"
    lines+=("$marker ${(V)_zghost_candidates[i]}")
  done
  lines+=("[$_zghost_index/$#_zghost_candidates] ↑/↓ select · Enter/Tab insert · Esc close")
  _zghost_render
  # Keep the menu in ZLE's transient display. `zle -M` prints persistent
  # output, so calling it from redraw hooks leaves duplicate menus behind.
  POSTDISPLAY+=$'\n'"${(F)lines}"
}
zghost-menu-dismiss() {
  (( ++_zghost_req )) # Invalidate any outstanding inline/menu response.
  _zghost_menu=0
  _zghost_candidates=()
  _zghost_candidate_cursors=()
  _zghost_clear_highlight
  POSTDISPLAY=
  zle -K $_zghost_keymap
}
zghost-menu-down() {
  (( _zghost_menu == 2 )) || return 0
  _zghost_index=$(( _zghost_index % $#_zghost_candidates + 1 ))
  _zghost_menu_draw
}
zghost-menu-up() {
  (( _zghost_menu == 2 )) || return 0
  _zghost_index=$(( (_zghost_index + $#_zghost_candidates - 2) % $#_zghost_candidates + 1 ))
  _zghost_menu_draw
}
zghost-menu-accept() {
  (( _zghost_menu == 2 )) || return 0
  local text=$_zghost_candidates[_zghost_index] at=$_zghost_candidate_cursors[_zghost_index]
  zghost-menu-dismiss
  BUFFER=$text
  CURSOR=$at
}
zghost-menu-type() {
  zghost-menu-dismiss
  zle .self-insert
}

# Use compsys as an on-demand source when built-in sources have no candidates.
# -A asks compadd for insertion strings without changing the line or listing.
_zghost_native_complete() {
  local _zghost_native_saved_compadd=${functions[compadd]-}
  function compadd() {
    # Completers use these output-array switches for their own intermediate
    # queries. Leave those alone, or their filtering logic would be disrupted.
    local -a _zghost_native_visible_prefix=() _zghost_native_visible_suffix=() _zghost_native_hidden_prefix=() _zghost_native_hidden_suffix=()
    local -a _zghost_native_files=() _zghost_native_file_root=() _zghost_native_outputs=() _zghost_native_ignored=() _zghost_native_ignored_prefix=() _zghost_native_ignored_suffix=() _zghost_native_unmatched=()
    zparseopts -E P:=_zghost_native_visible_prefix S:=_zghost_native_visible_suffix p:=_zghost_native_hidden_prefix s:=_zghost_native_hidden_suffix \
      f=_zghost_native_files W:=_zghost_native_file_root O:=_zghost_native_outputs A:=_zghost_native_outputs D:=_zghost_native_outputs \
      a=_zghost_native_ignored k=_zghost_native_ignored q=_zghost_native_ignored Q=_zghost_native_ignored e=_zghost_native_ignored n=_zghost_native_ignored U=_zghost_native_unmatched \
      l=_zghost_native_ignored 1=_zghost_native_ignored 2=_zghost_native_ignored C=_zghost_native_ignored F:=_zghost_native_ignored d:=_zghost_native_ignored \
      i:=_zghost_native_ignored_prefix I:=_zghost_native_ignored_suffix J:=_zghost_native_ignored V:=_zghost_native_ignored X:=_zghost_native_ignored x:=_zghost_native_ignored \
      r:=_zghost_native_ignored R:=_zghost_native_ignored M:=_zghost_native_ignored E:=_zghost_native_ignored
    if (( $#_zghost_native_outputs )); then
      builtin compadd "$@"
      return
    fi
    local -a _zghost_native_matches=()
    builtin compadd -A _zghost_native_matches "$@"
    local _zghost_native_result=$? _zghost_native_match _zghost_native_text _zghost_native_before=${_zghost_menu_buffer:0:$_zghost_word_start}
    local _zghost_native_after=${_zghost_menu_buffer:$_zghost_word_end}
    for _zghost_native_match in "$_zghost_native_matches[@]"; do
      local _zghost_native_leading=$IPREFIX
      (( $#_zghost_native_unmatched )) && _zghost_native_leading=
      local _zghost_native_word="$_zghost_native_leading${_zghost_native_ignored_prefix[2]}${_zghost_native_hidden_prefix[2]}$_zghost_native_match${_zghost_native_hidden_suffix[2]}${_zghost_native_ignored_suffix[2]}$ISUFFIX"
      local _zghost_native_file_path="${_zghost_native_file_root[2]}$IPREFIX${_zghost_native_hidden_prefix[2]}${(Q)_zghost_native_match}${_zghost_native_hidden_suffix[2]}"
      if (( $#_zghost_native_files )) && [[ -d $_zghost_native_file_path && $_zghost_native_word != */ ]]; then
        _zghost_native_word+=/
      fi
      _zghost_native_text="$_zghost_native_before${_zghost_native_visible_prefix[2]}${compstate[quote]}$_zghost_native_word${compstate[quote]}${_zghost_native_visible_suffix[2]}"
      [[ $_zghost_native_after == [[:space:]]* && $_zghost_native_text == *' ' ]] && _zghost_native_text=${_zghost_native_text% }
      if (( ! ${_zghost_candidates[(Ie)$_zghost_native_text$_zghost_native_after]} )); then
        _zghost_candidates+=("$_zghost_native_text$_zghost_native_after")
        _zghost_candidate_cursors+=($#_zghost_native_text)
      fi
      (( $#_zghost_candidates >= 100 )) && break
    done
    return $_zghost_native_result
  }
  {
    _main_complete
  } always {
    if [[ -n $_zghost_native_saved_compadd ]]; then
      functions[compadd]=$_zghost_native_saved_compadd
    else
      unfunction compadd
    fi
    compstate[insert]=''
    compstate[list]=''
  }
}

_zghost_preexec() {
  _zghost_cmd=$1
  _zghost_cmd_dir=$PWD
}

# Report the finished command and its exit status so ranking can learn.
_zghost_precmd() {
  local st=$?
  if [[ -n $_zghost_cmd && $_zghost_cmd != ' '* ]]; then
    local dir
    _zghost_encode $_zghost_cmd_dir; dir=$REPLY
    _zghost_encode $_zghost_cmd
    _zghost_send "R"$'\t'$$$'\t'$dir$'\t'$st$'\t'$REPLY
  fi
  _zghost_cmd=
}

zle -N _zghost-response
zle -N _zghost_pre_redraw
zle -N _zghost_line_init
zle -N _zghost_line_finish
zle -N zghost-accept
zle -N zghost-accept-end
zle -N zghost-accept-word
zle -N zghost-tab
zle -N zghost-next
zle -N zghost-prev
zle -N zghost-complete
zle -N zghost-menu-dismiss
zle -N zghost-menu-up
zle -N zghost-menu-down
zle -N zghost-menu-accept
zle -N zghost-menu-type
zle -C zghost-native-collect complete-word _zghost_native_complete
bindkey -N zghost-menu emacs
bindkey -M zghost-menu '^[[A' zghost-menu-up '^[OA' zghost-menu-up \
  '^[[B' zghost-menu-down '^[OB' zghost-menu-down \
  '^I' zghost-menu-accept '^M' zghost-menu-accept '^J' zghost-menu-accept \
  '^[' zghost-menu-dismiss '^G' zghost-menu-dismiss
bindkey -R -M zghost-menu ' '-'~' zghost-menu-type
add-zle-hook-widget line-init _zghost_line_init
add-zle-hook-widget line-pre-redraw _zghost_pre_redraw
add-zle-hook-widget line-finish _zghost_line_finish
add-zsh-hook preexec _zghost_preexec
# Run first so $? is still the command's exit status.
precmd_functions=(_zghost_precmd ${precmd_functions:#_zghost_precmd})

if [[ -z $ZGHOST_NO_BINDINGS ]]; then
  () {
    local km
    for km in emacs viins; do
      # Browse executed commands even when the current draft has no history
      # match. Some shell frameworks bind these to prefix search instead.
      bindkey -M $km '^[[A' up-line-or-history '^[OA' up-line-or-history \
        '^[[B' down-line-or-history '^[OB' down-line-or-history
      bindkey -M $km '^[[C' zghost-accept '^[OC' zghost-accept \
        '^E' zghost-accept-end '^[[F' zghost-accept-end '^[OF' zghost-accept-end
    done
    bindkey -M emacs '^F' zghost-accept '^[f' zghost-accept-word \
      '^[n' zghost-next '^[p' zghost-prev
  }

  # Bind after shell frameworks have set up their own completion widgets.
  _zghost_bind_tab() {
    add-zsh-hook -d precmd _zghost_bind_tab
    local current=${${(z)"$(bindkey '^I')"}[2]}
    [[ -n $current && $current != zghost-tab ]] && _zghost_tab_fallback=$current
    local km
    for km in emacs viins; do
      bindkey -M $km '^I' zghost-tab '^@' zghost-complete '^X^I' $_zghost_tab_fallback
    done
  }
  add-zsh-hook precmd _zghost_bind_tab
fi
