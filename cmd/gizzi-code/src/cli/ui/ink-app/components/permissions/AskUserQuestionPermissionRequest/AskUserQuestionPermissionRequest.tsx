import type { Base64ImageSource, ImageBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/messages.mjs';
import React, { Suspense, use, useCallback, useMemo, useRef, useState } from 'react';
import { useSettings } from '../../../hooks/useSettings';
import { useTerminalSize } from '../../../hooks/useTerminalSize';
import { stringWidth } from '../../../ink/stringWidth';
import { useTheme } from '../../../ink';
import { useKeybindings } from '../../../keybindings/useKeybinding';
import { type AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS, logEvent } from '../../../services/analytics/index';
import { useAppState } from '../../../state/AppState';
import type { Question } from '../../../tools/AskUserQuestionTool/AskUserQuestionTool';
import { AskUserQuestionTool } from '../../../tools/AskUserQuestionTool/AskUserQuestionTool';
import { type CliHighlight, getCliHighlightPromise } from '../../../utils/cliHighlight';
import type { PastedContent } from '../../../utils/config';
import type { ImageDimensions } from '../../../utils/imageResizer';
import { maybeResizeAndDownsampleImageBlock } from '../../../utils/imageResizer';
import { cacheImagePath, storeImage } from '../../../utils/imageStore';
import { logError } from '../../../utils/log';
import { applyMarkdown } from '../../../utils/markdown';
import { isPlanModeInterviewPhaseEnabled } from '../../../utils/planModeV2';
import { getPlanFilePath } from '../../../utils/plans';
import type { PermissionRequestProps } from '../PermissionRequest';
import { QuestionView } from './QuestionView';
import { SubmitQuestionsView } from './SubmitQuestionsView';
import { useMultipleChoiceState } from './use-multiple-choice-state';
import { expandQuestions, hasFollowUp, mergeAnswers } from './decision-tree';
const MIN_CONTENT_HEIGHT = 12;
const MIN_CONTENT_WIDTH = 40;
// Lines used by chrome around the content area (nav bar, title, footer, help text, etc.)
const CONTENT_CHROME_OVERHEAD = 15;
export function AskUserQuestionPermissionRequest(props) {
  const settings = useSettings();
  if (settings.syntaxHighlightingDisabled) {
    const t0 = <AskUserQuestionPermissionRequestBody {...props} highlight={null} />;

    return t0;
  }
  const t0 = <Suspense fallback={<AskUserQuestionPermissionRequestBody {...props} highlight={null} />}><AskUserQuestionWithHighlight {...props} /></Suspense>;

  return t0;
}
function AskUserQuestionWithHighlight(props) {
  const t0 = getCliHighlightPromise();

  const highlight = use(t0);
  const t1 = <AskUserQuestionPermissionRequestBody {...props} highlight={highlight} />;

  return t1;
}
function AskUserQuestionPermissionRequestBody(t0) {
  const {
    toolUseConfirm,
    onDone,
    onReject,
    highlight
  } = t0;
  const t1 = AskUserQuestionTool.inputSchema.safeParse(toolUseConfirm.input);

  const result = t1;
  const t2 = result.success ? result.data.questions || [] : [];

  const questions = t2;
  const {
    rows: terminalRows
  } = useTerminalSize();
  const [theme] = useTheme();
  let maxHeight = 0;
  let maxWidth = 0;
  const maxAllowedHeight = Math.max(MIN_CONTENT_HEIGHT, terminalRows - CONTENT_CHROME_OVERHEAD);
  for (const q of questions) {
    const hasPreview = q.options.some(_temp);
    if (hasPreview) {
      const maxPreviewContentLines = Math.max(1, maxAllowedHeight - 11);
      let maxPreviewBoxHeight = 0;
      for (const opt_0 of q.options) {
        if (opt_0.preview) {
          const rendered = applyMarkdown(opt_0.preview, theme, highlight);
          const previewLines = rendered.split("\n");
          const isTruncated = previewLines.length > maxPreviewContentLines;
          const displayedLines = isTruncated ? maxPreviewContentLines : previewLines.length;
          maxPreviewBoxHeight = Math.max(maxPreviewBoxHeight, displayedLines + (isTruncated ? 1 : 0) + 2);
          for (const line of previewLines) {
            maxWidth = Math.max(maxWidth, stringWidth(line));
          }
        }
      }
      const rightPanelHeight = maxPreviewBoxHeight + 2;
      const leftPanelHeight = q.options.length + 2;
      const sideByHeight = Math.max(leftPanelHeight, rightPanelHeight);
      maxHeight = Math.max(maxHeight, sideByHeight + 7);
    } else {
      maxHeight = Math.max(maxHeight, q.options.length + 3 + 7);
    }
  }

  const t3 = Math.min(Math.max(maxHeight, MIN_CONTENT_HEIGHT), maxAllowedHeight);
  const t4 = Math.max(maxWidth, MIN_CONTENT_WIDTH);
  const t5 = {
      globalContentHeight: t3,
      globalContentWidth: t4
    };

  const {
    globalContentHeight,
    globalContentWidth
  } = t5;
  const metadataSource = result.success ? result.data.metadata?.source : undefined;
  const t6: Record<string, Record<number, PastedContent>> = {};

  const [pastedContentsByQuestion, setPastedContentsByQuestion] = useState<Record<string, Record<number, PastedContent>>>(t6);
  const nextPasteIdRef = useRef(0);
  const t7 = function onImagePaste(questionText, base64Image, mediaType, filename, dimensions, _sourcePath) {
      nextPasteIdRef.current = nextPasteIdRef.current + 1;
      const pasteId = nextPasteIdRef.current;
      const newContent: PastedContent = {
        id: pasteId,
        type: "image",
        content: base64Image,
        mediaType: mediaType || "image/png",
        filename: filename || "Pasted image",
        dimensions
      };
      cacheImagePath(newContent);
      storeImage(newContent);
      setPastedContentsByQuestion(prev => ({
        ...prev,
        [questionText]: {
          ...(prev[questionText] ?? {}),
          [pasteId]: newContent
        }
      }));
    };

  const onImagePaste = t7;
  const t8 = (questionText_0, id) => {
      setPastedContentsByQuestion(prev_0 => {
        const questionContents = {
          ...(prev_0[questionText_0] ?? {})
        };
        delete questionContents[id];
        return {
          ...prev_0,
          [questionText_0]: questionContents
        };
      });
    };

  const onRemoveImage = t8;
  const t9: PastedContent[] = Object.values(pastedContentsByQuestion).flatMap(_temp2).filter(_temp3);

  const allImageAttachments = t9;
  const toolPermissionContextMode = useAppState(_temp4);
  const isInPlanMode = toolPermissionContextMode === "plan";
  const t10 = isInPlanMode ? getPlanFilePath() : undefined;

  const planFilePath = t10;
  const state = useMultipleChoiceState();
  const {
    currentQuestionIndex,
    answers,
    questionStates,
    isInTextInput,
    nextQuestion,
    prevQuestion,
    updateQuestionState,
    setAnswer,
    setTextInputMode
  } = state;
  // Decision trees: a picked branch with `then` adds its follow-up right after it.
  const shownQuestions = expandQuestions(questions, answers).display;
  const currentQuestion = currentQuestionIndex < shownQuestions.length ? shownQuestions[currentQuestionIndex] : null;
  const isInSubmitView = currentQuestionIndex === shownQuestions.length;
  const t11 = shownQuestions.every(q_0 => q_0?.question && !!answers[q_0.question]);

  const allQuestionsAnswered = t11;
  const hideSubmitTab = questions.length === 1 && !questions[0]?.multiSelect;
  const t12 = () => {
      if (metadataSource) {
        logEvent("tengu_ask_user_question_rejected", {
          source: metadataSource as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS,
          questionCount: questions.length,
          isInPlanMode,
          interviewPhaseEnabled: isInPlanMode && isPlanModeInterviewPhaseEnabled()
        });
      }
      onDone();
      onReject();
      toolUseConfirm.onReject();
    };

  const handleCancel = t12;
  const t13 = async () => {
      const mergedAnswers = mergeAnswers(questions, answers);
      const questionsWithAnswers = questions.map(q_1 => {
        const answer = mergedAnswers[q_1.question];
        if (answer) {
          return `- "${q_1.question}"\n  Answer: ${answer}`;
        }
        return `- "${q_1.question}"\n  (No answer provided)`;
      }).join("\n");
      const feedback = `The user wants to clarify these questions.
    This means they may have additional information, context or questions for you.
    Take their response into account and then reformulate the questions if appropriate.
    Start by asking them what they would like to clarify.

    Questions asked:\n${questionsWithAnswers}`;
      if (metadataSource) {
        logEvent("tengu_ask_user_question_respond_to_claude", {
          source: metadataSource as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS,
          questionCount: questions.length,
          isInPlanMode,
          interviewPhaseEnabled: isInPlanMode && isPlanModeInterviewPhaseEnabled()
        });
      }
      const imageBlocks = await convertImagesToBlocks(allImageAttachments);
      onDone();
      toolUseConfirm.onReject(feedback, imageBlocks && imageBlocks.length > 0 ? imageBlocks : undefined);
    };

  const handleRespondToClaude = t13;
  const t14 = async () => {
      const mergedAnswers_0 = mergeAnswers(questions, answers);
      const questionsWithAnswers_0 = questions.map(q_2 => {
        const answer_0 = mergedAnswers_0[q_2.question];
        if (answer_0) {
          return `- "${q_2.question}"\n  Answer: ${answer_0}`;
        }
        return `- "${q_2.question}"\n  (No answer provided)`;
      }).join("\n");
      const feedback_0 = `The user has indicated they have provided enough answers for the plan interview.
Stop asking clarifying questions and proceed to finish the plan with the information you have.

Questions asked and answers provided:\n${questionsWithAnswers_0}`;
      if (metadataSource) {
        logEvent("tengu_ask_user_question_finish_plan_interview", {
          source: metadataSource as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS,
          questionCount: questions.length,
          isInPlanMode,
          interviewPhaseEnabled: isInPlanMode && isPlanModeInterviewPhaseEnabled()
        });
      }
      const imageBlocks_0 = await convertImagesToBlocks(allImageAttachments);
      onDone();
      toolUseConfirm.onReject(feedback_0, imageBlocks_0 && imageBlocks_0.length > 0 ? imageBlocks_0 : undefined);
    };

  const handleFinishPlanInterview = t14;
  const t15 = async shownAnswers => {
      const answersToSubmit = mergeAnswers(questions, shownAnswers);
      if (metadataSource) {
        logEvent("tengu_ask_user_question_accepted", {
          source: metadataSource as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS,
          questionCount: questions.length,
          answerCount: Object.keys(answersToSubmit).length,
          isInPlanMode,
          interviewPhaseEnabled: isInPlanMode && isPlanModeInterviewPhaseEnabled()
        });
      }
      const annotations = {};
      for (const q_3 of questions) {
        const answer_1 = answersToSubmit[q_3.question];
        const notes = questionStates[q_3.question]?.textInputValue;
        const selectedOption = answer_1 ? q_3.options.find(opt_1 => opt_1.label === answer_1) : undefined;
        const preview = selectedOption?.preview;
        if (preview || notes?.trim()) {
          annotations[q_3.question] = {
            ...(preview && {
              preview
            }),
            ...(notes?.trim() && {
              notes: notes.trim()
            })
          };
        }
      }
      const updatedInput = {
        ...toolUseConfirm.input,
        answers: answersToSubmit,
        ...(Object.keys(annotations).length > 0 && {
          annotations
        })
      };
      const contentBlocks = await convertImagesToBlocks(allImageAttachments);
      onDone();
      toolUseConfirm.onAllow(updatedInput, [], undefined, contentBlocks && contentBlocks.length > 0 ? contentBlocks : undefined);
    };

  const submitAnswers = t15;
  const t16 = (questionText_1, label, textInput, t17) => {
      const shouldAdvance = t17 === undefined ? true : t17;
      let answer_2;
      const isMultiSelect = Array.isArray(label);
      if (isMultiSelect) {
        answer_2 = label.join(", ");
      } else {
        if (textInput) {
          const questionImages = Object.values(pastedContentsByQuestion[questionText_1] ?? {}).filter(_temp5);
          answer_2 = questionImages.length > 0 ? `${textInput} (Image attached)` : textInput;
        } else {
          if (label === "__other__") {
            const questionImages_0 = Object.values(pastedContentsByQuestion[questionText_1] ?? {}).filter(_temp6);
            answer_2 = questionImages_0.length > 0 ? "(Image attached)" : label;
          } else {
            answer_2 = label;
          }
        }
      }
      const isSingleQuestion = questions.length === 1;
      const answeredRoot = questions.find(q_4 => q_4.question === questionText_1);
      if (!isMultiSelect && shouldAdvance && answeredRoot && hasFollowUp(answeredRoot, answer_2)) {
        // Show the branch's follow-up question next instead of submitting.
        setAnswer(questionText_1, answer_2, true);
        return;
      }
      if (!isMultiSelect && isSingleQuestion && shouldAdvance) {
        const updatedAnswers = {
          ...answers,
          [questionText_1]: answer_2
        };
        submitAnswers(updatedAnswers).catch(logError);
        return;
      }
      setAnswer(questionText_1, answer_2, shouldAdvance);
    };

  const handleQuestionAnswer = t16;
  const t17 = function handleFinalResponse(value) {
      if (value === "cancel") {
        handleCancel();
        return;
      }
      if (value === "submit") {
        submitAnswers(answers).catch(logError);
      }
    };

  const handleFinalResponse = t17;
  const maxIndex = hideSubmitTab ? shownQuestions.length - 1 : shownQuestions.length;
  const t18 = () => {
      if (currentQuestionIndex > 0) {
        prevQuestion();
      }
    };

  const handleTabPrev = t18;
  const t19 = () => {
      if (currentQuestionIndex < maxIndex) {
        nextQuestion();
      }
    };

  const handleTabNext = t19;
  const t20 = {
      "tabs:previous": handleTabPrev,
      "tabs:next": handleTabNext
    };

  const t21 = !(isInTextInput && !isInSubmitView);
  const t22 = {
      context: "Tabs",
      isActive: t21
    };

  useKeybindings(t20, t22);
  if (currentQuestion) {
    const t23 = (base64, mediaType_0, filename_0, dims, path) => onImagePaste(currentQuestion.question, base64, mediaType_0, filename_0, dims, path);

    const t24 = pastedContentsByQuestion[currentQuestion.question] ?? {};

    const t25 = id_0 => onRemoveImage(currentQuestion.question, id_0);

    const t26 = <><QuestionView question={currentQuestion} questions={shownQuestions} currentQuestionIndex={currentQuestionIndex} answers={answers} questionStates={questionStates} hideSubmitTab={hideSubmitTab} minContentHeight={globalContentHeight} minContentWidth={globalContentWidth} planFilePath={planFilePath} onUpdateQuestionState={updateQuestionState} onAnswer={handleQuestionAnswer} onTextInputFocus={setTextInputMode} onCancel={handleCancel} onSubmit={nextQuestion} onTabPrev={handleTabPrev} onTabNext={handleTabNext} onRespondToClaude={handleRespondToClaude} onFinishPlanInterview={handleFinishPlanInterview} onImagePaste={t23} pastedContents={t24} onRemoveImage={t25} /></>;

    return t26;
  }
  if (isInSubmitView) {
    const t23 = <><SubmitQuestionsView questions={shownQuestions} currentQuestionIndex={currentQuestionIndex} answers={answers} allQuestionsAnswered={allQuestionsAnswered} permissionResult={toolUseConfirm.permissionResult} minContentHeight={globalContentHeight} onFinalResponse={handleFinalResponse} /></>;

    return t23;
  }
  return null;
}
function _temp6(c_1) {
  return c_1.type === "image";
}
function _temp5(c_0) {
  return c_0.type === "image";
}
function _temp4(s) {
  return s.toolPermissionContext.mode;
}
function _temp3(c: PastedContent) {
  return c.type === "image";
}
function _temp2(contents: Record<number, PastedContent>): PastedContent[] {
  return Object.values(contents);
}
function _temp(opt) {
  return opt.preview;
}
async function convertImagesToBlocks(images: PastedContent[]): Promise<ImageBlockParam[] | undefined> {
  if (images.length === 0) return undefined;
  return Promise.all(images.map(async img => {
    const block: ImageBlockParam = {
      type: 'image',
      source: {
        type: 'base64',
        media_type: (img.mediaType || 'image/png') as Base64ImageSource['media_type'],
        data: img.content
      }
    };
    const resized = await maybeResizeAndDownsampleImageBlock(block);
    return resized.block;
  }));
}
