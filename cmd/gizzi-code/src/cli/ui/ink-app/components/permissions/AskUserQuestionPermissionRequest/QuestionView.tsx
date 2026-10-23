import figures from 'figures';
import React, { useCallback, useState } from 'react';
import type { KeyboardEvent } from '../../../ink/events/keyboard-event';
import { Box, Text } from '../../../ink';
import { useAppState } from '../../../state/AppState';
import type { Question, QuestionOption } from '../../../tools/AskUserQuestionTool/AskUserQuestionTool';
import type { PastedContent } from '../../../utils/config';
import { getExternalEditor } from '../../../utils/editor';
import { toIDEDisplayName } from '../../../utils/ide';
import type { ImageDimensions } from '../../../utils/imageResizer';
import { editPromptInEditor } from '../../../utils/promptEditor';
import { type OptionWithDescription, Select, SelectMulti } from '../../CustomSelect/index';
import { Divider } from '../../design-system/Divider';
import { FilePathLink } from '../../FilePathLink';
import { PermissionRequestTitle } from '../PermissionRequestTitle';
import { PreviewQuestionView } from './PreviewQuestionView';
import { DecisionContext } from './DecisionContext';
import { optionDescription, optionLabel } from './decision-tree';
import { QuestionNavigationBar } from './QuestionNavigationBar';
import type { QuestionState } from './use-multiple-choice-state';
type Props = {
  question: Question;
  questions: Question[];
  currentQuestionIndex: number;
  answers: Record<string, string>;
  questionStates: Record<string, QuestionState>;
  hideSubmitTab?: boolean;
  planFilePath?: string;
  pastedContents?: Record<number, PastedContent>;
  minContentHeight?: number;
  minContentWidth?: number;
  onUpdateQuestionState: (questionText: string, updates: Partial<QuestionState>, isMultiSelect: boolean) => void;
  onAnswer: (questionText: string, label: string | string[], textInput?: string, shouldAdvance?: boolean) => void;
  onTextInputFocus: (isInInput: boolean) => void;
  onCancel: () => void;
  onSubmit: () => void;
  onTabPrev?: () => void;
  onTabNext?: () => void;
  onRespondToClaude: () => void;
  onFinishPlanInterview: () => void;
  onImagePaste?: (base64Image: string, mediaType?: string, filename?: string, dimensions?: ImageDimensions, sourcePath?: string) => void;
  onRemoveImage?: (id: number) => void;
};
export function QuestionView({
    question,
    questions,
    currentQuestionIndex,
    answers,
    questionStates,
    hideSubmitTab: t1,
    planFilePath,
    minContentHeight,
    minContentWidth,
    onUpdateQuestionState,
    onAnswer,
    onTextInputFocus,
    onCancel,
    onSubmit,
    onTabPrev,
    onTabNext,
    onRespondToClaude,
    onFinishPlanInterview,
    onImagePaste,
    pastedContents,
    onRemoveImage
}: Props) {
  const hideSubmitTab = t1 === undefined ? false : t1;
  const isInPlanMode = useAppState(_temp) === "plan";
  const [isFooterFocused, setIsFooterFocused] = useState(false);
  const [footerIndex, setFooterIndex] = useState(0);
  const [isOtherFocused, setIsOtherFocused] = useState(false);
  const editor = getExternalEditor();
  const t2 = editor ? toIDEDisplayName(editor) : null;

  const editorName = t2;
  const t3 = value => {
      const isOther = value === "__other__";
      setIsOtherFocused(isOther);
      onTextInputFocus(isOther);
    };

  const handleFocus = t3;
  const t4 = () => {
      setIsFooterFocused(true);
    };

  const handleDownFromLastItem = t4;
  const t5 = () => {
      setIsFooterFocused(false);
    };

  const handleUpFromFooter = t5;
  const t6 = e => {
      if (!isFooterFocused) {
        return;
      }
      if (e.key === "up" || e.ctrl && e.key === "p") {
        e.preventDefault();
        if (footerIndex === 0) {
          handleUpFromFooter();
        } else {
          setFooterIndex(0);
        }
        return;
      }
      if (e.key === "down" || e.ctrl && e.key === "n") {
        e.preventDefault();
        if (isInPlanMode && footerIndex === 0) {
          setFooterIndex(1);
        }
        return;
      }
      if (e.key === "return") {
        e.preventDefault();
        if (footerIndex === 0) {
          onRespondToClaude();
        } else {
          onFinishPlanInterview();
        }
        return;
      }
      if (e.key === "escape") {
        e.preventDefault();
        onCancel();
      }
    };

  const handleKeyDown = t6;
  const textOptions = question.options.map(_temp2);
  const questionText = question.question;
  const questionState = questionStates[questionText];
  const t8 = async (currentValue, setValue) => {
        const result = await editPromptInEditor(currentValue);
        if (result.content !== null && result.content !== currentValue) {
          setValue(result.content);
          onUpdateQuestionState(questionText, {
            textInputValue: result.content
          }, question.multiSelect ?? false);
        }
      };

  const handleOpenEditor = t8;
  const t9 = question.multiSelect ? "Type something" : "Type something.";
  const t10 = questionState?.textInputValue ?? "";
  const t11 = value_0 => {
        onUpdateQuestionState(questionText, {
          textInputValue: value_0
        }, question.multiSelect ?? false);
      };

  const t12 = {
        type: "input" as const,
        value: "__other__",
        label: "Other",
        placeholder: t9,
        initialValue: t10,
        onChange: t11
      };

  const otherOption = t12;
  const t7 = [...textOptions, otherOption];

  const options = t7;
  const hasAnyPreview = !question.multiSelect && question.options.some(_temp3);
  if (hasAnyPreview) {
    const t8 = <PreviewQuestionView question={question} questions={questions} currentQuestionIndex={currentQuestionIndex} answers={answers} questionStates={questionStates} hideSubmitTab={hideSubmitTab} minContentHeight={minContentHeight} minContentWidth={minContentWidth} onUpdateQuestionState={onUpdateQuestionState} onAnswer={onAnswer} onTextInputFocus={onTextInputFocus} onCancel={onCancel} onTabPrev={onTabPrev} onTabNext={onTabNext} onRespondToClaude={onRespondToClaude} onFinishPlanInterview={onFinishPlanInterview} />;

    return t8;
  }
  const t8_2 = isInPlanMode && planFilePath && <Box flexDirection="column" gap={0}><Divider color="inactive" /><Text color="inactive">Planning: <FilePathLink filePath={planFilePath} /></Text></Box>;

  const t9_2 = <Box marginTop={-1}><Divider color="inactive" /></Box>;

  const t10_2 = <QuestionNavigationBar questions={questions} currentQuestionIndex={currentQuestionIndex} answers={answers} hideSubmitTab={hideSubmitTab} />;

  const t11_2 = <><PermissionRequestTitle title={question.question} color="text" /><DecisionContext question={question} /></>;

  const t12_2 = <Box marginTop={1}>{question.multiSelect ? <SelectMulti key={question.question} options={options} defaultValue={questionStates[question.question]?.selectedValue as string[] | undefined} onChange={values => {
        onUpdateQuestionState(questionText, {
          selectedValue: values
        }, true);
        const textInput = values.includes("__other__") ? questionStates[questionText]?.textInputValue : undefined;
        const finalValues = values.filter(_temp4).concat(textInput ? [textInput] : []);
        onAnswer(questionText, finalValues, undefined, false);
      }} onFocus={handleFocus} onCancel={onCancel} submitButtonText={currentQuestionIndex === questions.length - 1 ? "Submit" : "Next"} onSubmit={onSubmit} onDownFromLastItem={handleDownFromLastItem} isDisabled={isFooterFocused} onOpenEditor={handleOpenEditor} onImagePaste={onImagePaste} pastedContents={pastedContents} onRemoveImage={onRemoveImage} /> : <Select key={question.question} options={options} defaultValue={questionStates[question.question]?.selectedValue as string | undefined} onChange={value_1 => {
        onUpdateQuestionState(questionText, {
          selectedValue: value_1
        }, false);
        const textInput_0 = value_1 === "__other__" ? questionStates[questionText]?.textInputValue : undefined;
        onAnswer(questionText, value_1, textInput_0);
      }} onFocus={handleFocus} onCancel={onCancel} onDownFromLastItem={handleDownFromLastItem} isDisabled={isFooterFocused} layout="compact-vertical" onOpenEditor={handleOpenEditor} onImagePaste={onImagePaste} pastedContents={pastedContents} onRemoveImage={onRemoveImage} />}</Box>;

  const t13 = <Divider color="inactive" />;

  const t14 = isFooterFocused && footerIndex === 0 ? <Text color="suggestion">{figures.pointer}</Text> : <Text> </Text>;

  const t15 = isFooterFocused && footerIndex === 0 ? "suggestion" : undefined;
  const t16 = options.length + 1;
  const t17 = <Text color={t15}>{t16}. Chat about this</Text>;

  const t18 = <Box flexDirection="row" gap={1}>{t14}{t17}</Box>;

  const t19 = isInPlanMode && <Box flexDirection="row" gap={1}>{isFooterFocused && footerIndex === 1 ? <Text color="suggestion">{figures.pointer}</Text> : <Text> </Text>}<Text color={isFooterFocused && footerIndex === 1 ? "suggestion" : undefined}>{options.length + 2}. Skip interview and plan immediately</Text></Box>;

  const t20 = <Box flexDirection="column">{t13}{t18}{t19}</Box>;

  const t21 = questions.length === 1 ? <>{figures.arrowUp}/{figures.arrowDown} to navigate</> : "Tab/Arrow keys to navigate";

  const t22 = isOtherFocused && editorName && <> · ctrl+g to edit in {editorName}</>;

  const t23 = <Box marginTop={1}><Text color="inactive" dimColor={true}>Enter to select ·{" "}{t21}{t22}{" "}· Esc to cancel</Text></Box>;

  const t24 = <Box flexDirection="column" minHeight={minContentHeight}>{t12_2}{t20}{t23}</Box>;

  const t25 = <Box flexDirection="column" paddingTop={0}>{t10_2}{t11_2}{t24}</Box>;

  const t26 = <Box flexDirection="column" marginTop={0} tabIndex={0} autoFocus={true} onKeyDown={handleKeyDown}>{t8_2}{t9_2}{t25}</Box>;

  return t26;
}
function _temp4(v) {
  return v !== "__other__";
}
function _temp3(opt_0) {
  return opt_0.preview;
}
function _temp2(opt) {
  return {
    type: "text" as const,
    value: opt.label,
    label: optionLabel(opt),
    description: optionDescription(opt)
  };
}
function _temp(s) {
  return s.toolPermissionContext.mode;
}
