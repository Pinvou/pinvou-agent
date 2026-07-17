export function createPetCardUiState() {
  return {
    expandedSessionId: null,
    replySessionId: null,
    draft: '',
    pendingRequestId: null,
    retrySubmission: null,
    error: '',
  };
}

export function normalizedPetReply(text) {
  return String(text ?? '').trim();
}

export function petCardUiReducer(state, action) {
  switch (action.type) {
    case 'toggle-expand':
      return {
        ...state,
        expandedSessionId:
          state.expandedSessionId === action.sessionId ? null : action.sessionId,
      };

    case 'open-reply':
      return {
        ...state,
        replySessionId: action.sessionId,
        draft: '',
        pendingRequestId: null,
        retrySubmission: null,
        error: '',
      };

    case 'edit-reply':
      return { ...state, draft: action.text, error: '' };

    case 'submit-reply':
      return { ...state, pendingRequestId: action.requestId, error: '' };

    case 'reply-accepted':
      if (state.pendingRequestId !== action.requestId) return state;
      return {
        ...state,
        retrySubmission: {
          requestId: action.requestId,
          sessionId: state.replySessionId,
          text: state.draft,
        },
        replySessionId: null,
        draft: '',
        pendingRequestId: null,
        error: '',
      };

    case 'reply-failed':
      if (state.pendingRequestId !== action.requestId) {
        if (state.retrySubmission?.requestId !== action.requestId) return state;
        return {
          ...state,
          replySessionId: state.retrySubmission.sessionId,
          draft: state.retrySubmission.text,
          retrySubmission: null,
          error: action.error || '发送失败',
        };
      }
      return {
        ...state,
        pendingRequestId: null,
        retrySubmission: null,
        error: action.error || '发送失败',
      };

    case 'close-reply':
      return {
        ...state,
        replySessionId: null,
        draft: '',
        pendingRequestId: null,
        retrySubmission: null,
        error: '',
      };

    case 'dismiss': {
      const retryingSession = state.retrySubmission?.sessionId === action.sessionId;
      if (
        state.expandedSessionId !== action.sessionId &&
        state.replySessionId !== action.sessionId &&
        !retryingSession
      ) {
        return state;
      }
      return {
        ...state,
        expandedSessionId:
          state.expandedSessionId === action.sessionId ? null : state.expandedSessionId,
        replySessionId: state.replySessionId === action.sessionId ? null : state.replySessionId,
        draft: state.replySessionId === action.sessionId ? '' : state.draft,
        pendingRequestId:
          state.replySessionId === action.sessionId ? null : state.pendingRequestId,
        retrySubmission: retryingSession ? null : state.retrySubmission,
        error: state.replySessionId === action.sessionId ? '' : state.error,
      };
    }

    default:
      return state;
  }
}
