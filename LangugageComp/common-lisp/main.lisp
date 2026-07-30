(defpackage #:buildgraph
  (:use #:cl)
  (:export #:main))

(in-package #:buildgraph)

(defparameter +help+
  (format nil
          "BuildGraph - deterministic dependency graph analyzer~%~%~
           usage:~%~
           ~2Tbuildgraph analyze <file>~%~
           ~2Tbuildgraph affected <file> <task>~%~
           ~2Tbuildgraph --help~%"))

(defparameter +usage-error+
  "expected 'analyze <file>' or 'affected <file> <task>'")

(defconstant +maximum-duration+ 2147483647)
(defconstant +maximum-i64+ 9223372036854775807)

(define-condition buildgraph-input-error (error)
  ((message :initarg :message :reader buildgraph-input-error-message)))

(defstruct task
  name
  duration
  line
  dependency-spec)

(defstruct graph
  tasks
  edge-from
  edge-to
  indegree
  indexes)

(defun fail-input (control &rest arguments)
  (error 'buildgraph-input-error
         :message (apply #'format nil control arguments)))

(defun ascii-letter-p (character)
  (let ((code (char-code character)))
    (or (<= (char-code #\A) code (char-code #\Z))
        (<= (char-code #\a) code (char-code #\z)))))

(defun ascii-digit-p (character)
  (let ((code (char-code character)))
    (<= (char-code #\0) code (char-code #\9))))

(defun identifier-p (value)
  (let ((length (length value)))
    (and (<= 1 length 32)
         (ascii-letter-p (char value 0))
         (loop for index from 1 below length
               for character = (char value index)
               always (or (ascii-letter-p character)
                          (ascii-digit-p character)
                          (char= character #\_)
                          (char= character #\-))))))

(defun trim-whitespace (value)
  (string-trim '(#\Space #\Tab #\Return #\Newline) value))

(defun split-on-character (value separator)
  (loop with start = 0
        for end = (position separator value :start start)
        collect (subseq value start end)
        while end
        do (setf start (1+ end))))

(defun make-growable-vector ()
  (make-array 0 :adjustable t :fill-pointer 0))

(defun parse-duration (value line)
  (unless (and (> (length value) 0)
               (every #'ascii-digit-p value))
    (fail-input "line ~D: invalid duration '~A'" line value))
  (let ((duration (parse-integer value)))
    (unless (<= 1 duration +maximum-duration+)
      (fail-input "line ~D: invalid duration '~A'" line value))
    duration))

(defun parse-records (text)
  (let ((tasks (make-growable-vector))
        (indexes (make-hash-table :test #'equal)))
    (loop for raw-line in (split-on-character text #\Newline)
          for line-number from 1
          for line = (trim-whitespace raw-line)
          unless (or (zerop (length line))
                     (char= (char line 0) #\#))
            do
               (let ((fields (split-on-character line #\|)))
                 (unless (= (length fields) 3)
                   (fail-input
                    "line ~D: expected exactly three '|' separated fields"
                    line-number))
                 (let* ((name (trim-whitespace (first fields)))
                        (duration-text (trim-whitespace (second fields)))
                        (dependency-spec (trim-whitespace (third fields))))
                   (unless (identifier-p name)
                     (fail-input "line ~D: invalid task identifier '~A'"
                                 line-number name))
                   (multiple-value-bind (existing present-p)
                       (gethash name indexes)
                     (declare (ignore existing))
                     (when present-p
                       (fail-input "line ~D: duplicate task '~A'"
                                   line-number name)))
                   (let ((index (length tasks)))
                     (setf (gethash name indexes) index)
                     (vector-push-extend
                      (make-task
                       :name name
                       :duration (parse-duration duration-text line-number)
                       :line line-number
                       :dependency-spec dependency-spec)
                      tasks)))))
    (when (zerop (length tasks))
      (fail-input "no tasks"))
    (values tasks indexes)))

(defun build-edges (tasks indexes)
  (let ((edge-from (make-growable-vector))
        (edge-to (make-growable-vector))
        (indegree (make-array (length tasks) :initial-element 0)))
    (loop for task-index from 0 below (length tasks)
          for task = (aref tasks task-index)
          for dependency-spec = (task-dependency-spec task)
          unless (zerop (length dependency-spec))
            do
               (let ((seen (make-hash-table :test #'equal)))
                 (dolist (raw-dependency
                          (split-on-character dependency-spec #\,))
                   (let ((dependency (trim-whitespace raw-dependency)))
                     (when (zerop (length dependency))
                       (fail-input
                        "line ~D: empty dependency for task '~A'"
                        (task-line task) (task-name task)))
                     (unless (identifier-p dependency)
                       (fail-input
                        "line ~D: invalid dependency identifier '~A'"
                        (task-line task) dependency))
                     (when (string= dependency (task-name task))
                       (fail-input
                        "line ~D: task '~A' depends on itself"
                        (task-line task) (task-name task)))
                     (when (gethash dependency seen)
                       (fail-input
                        "line ~D: duplicate dependency '~A' for task '~A'"
                        (task-line task) dependency (task-name task)))
                     (setf (gethash dependency seen) t)
                     (multiple-value-bind (dependency-index present-p)
                         (gethash dependency indexes)
                       (unless present-p
                         (fail-input
                          "line ~D: unknown dependency '~A' for task '~A'"
                          (task-line task) dependency (task-name task)))
                       (vector-push-extend dependency-index edge-from)
                       (vector-push-extend task-index edge-to)
                       (incf (aref indegree task-index)))))))
    (values edge-from edge-to indegree)))

(defun parse-graph (text)
  (multiple-value-bind (tasks indexes)
      (parse-records text)
    (multiple-value-bind (edge-from edge-to indegree)
        (build-edges tasks indexes)
      (make-graph
       :tasks tasks
       :edge-from edge-from
       :edge-to edge-to
       :indegree indegree
       :indexes indexes))))

(defun stable-topological-order (graph)
  (let* ((tasks (graph-tasks graph))
         (count (length tasks))
         (indegree (copy-seq (graph-indegree graph)))
         (processed (make-array count :initial-element nil))
         (order (make-growable-vector)))
    (loop while (< (length order) count)
          do
             (let ((selected
                     (loop for index from 0 below count
                           when (and (not (aref processed index))
                                     (zerop (aref indegree index)))
                             return index)))
               (unless selected
                 (fail-input "cycle detected"))
               (setf (aref processed selected) t)
               (vector-push-extend selected order)
               (loop for edge from 0 below (length (graph-edge-from graph))
                     when (= (aref (graph-edge-from graph) edge) selected)
                       do (decf
                           (aref indegree
                                 (aref (graph-edge-to graph) edge))))))
    order))

(defun lexicographically-earlier-p (left right)
  (loop for left-index in left
        for right-index in right
        when (< left-index right-index)
          return t
        when (> left-index right-index)
          return nil
        finally (return (< (length left) (length right)))))

(defun critical-path (graph order)
  (let* ((tasks (graph-tasks graph))
         (count (length tasks))
         (distance (make-array count :initial-element 0))
         (paths (make-array count :initial-element nil)))
    (loop for task-index across order
          for task = (aref tasks task-index)
          do
             (let ((best-distance (task-duration task))
                   (best-path (list task-index)))
               (loop for edge from 0 below (length (graph-edge-from graph))
                     when (= (aref (graph-edge-to graph) edge) task-index)
                       do
                          (let* ((dependency
                                   (aref (graph-edge-from graph) edge))
                                 (candidate-distance
                                   (+ (aref distance dependency)
                                      (task-duration task)))
                                 (candidate-path
                                   (append (aref paths dependency)
                                           (list task-index))))
                            (when (> candidate-distance +maximum-i64+)
                              (fail-input "critical duration overflow"))
                            (when (or (> candidate-distance best-distance)
                                      (and (= candidate-distance best-distance)
                                           (lexicographically-earlier-p
                                            candidate-path best-path)))
                              (setf best-distance candidate-distance
                                    best-path candidate-path))))
               (setf (aref distance task-index) best-distance
                     (aref paths task-index) best-path)))
    (let ((best-end (aref order 0)))
      (loop for position from 1 below (length order)
            for task-index = (aref order position)
            when (or (> (aref distance task-index)
                        (aref distance best-end))
                     (and (= (aref distance task-index)
                             (aref distance best-end))
                          (lexicographically-earlier-p
                           (aref paths task-index)
                           (aref paths best-end))))
              do (setf best-end task-index))
      (values (aref distance best-end)
              (aref paths best-end)))))

(defun join-task-names (tasks indexes separator)
  (with-output-to-string (output)
    (loop for index in indexes
          for first-p = t then nil
          unless first-p
            do (write-string separator output)
          do (write-string (task-name (aref tasks index)) output))))

(defun vector-as-list (vector)
  (loop for value across vector collect value))

(defun analyze-graph (graph)
  (let ((order (stable-topological-order graph)))
    (multiple-value-bind (duration path)
        (critical-path graph order)
      (format nil
              "tasks: ~D~%order: ~A~%critical-duration: ~D~%critical-path: ~A~%"
              (length (graph-tasks graph))
              (join-task-names
               (graph-tasks graph) (vector-as-list order) ", ")
              duration
              (join-task-names (graph-tasks graph) path " -> ")))))

(defun affected-tasks (graph task-name)
  (multiple-value-bind (query present-p)
      (gethash task-name (graph-indexes graph))
    (unless present-p
      (fail-input "unknown task '~A'" task-name))
    (let* ((order (stable-topological-order graph))
           (marked (make-array (length (graph-tasks graph))
                               :initial-element nil)))
      (loop for task-index across order
            do
               (setf (aref marked task-index)
                     (or (= task-index query)
                         (loop for edge from 0
                                 below (length (graph-edge-from graph))
                               thereis
                               (and (= (aref (graph-edge-to graph) edge)
                                       task-index)
                                    (aref marked
                                          (aref (graph-edge-from graph)
                                                edge)))))))
      (let ((affected
              (loop for task-index across order
                    when (aref marked task-index)
                      collect task-index)))
        (format nil "affected: ~A~%"
                (join-task-names (graph-tasks graph) affected ", "))))))

(defun read-utf8-file (path)
  (with-open-file (input path :direction :input :external-format :utf-8)
    (with-output-to-string (output)
      (loop for character = (read-char input nil nil)
            while character
            do (write-char character output)))))

(defun exit-with-error (prefix message code)
  (format *error-output* "~A: ~A~%" prefix message)
  (finish-output *error-output*)
  (sb-ext:exit :code code))

(defun application-arguments ()
  (rest sb-ext:*posix-argv*))

(defun main ()
  (let ((arguments (application-arguments)))
    (when (and (= (length arguments) 1)
               (member (first arguments) '("--help" "-h")
                       :test #'string=))
      (write-string +help+)
      (finish-output)
      (sb-ext:exit :code 0))
    (let (command path query)
      (cond
        ((and (= (length arguments) 2)
              (string= (first arguments) "analyze"))
         (setf command "analyze"
               path (second arguments)))
        ((and (= (length arguments) 3)
              (string= (first arguments) "affected"))
         (setf command "affected"
               path (second arguments)
               query (third arguments)))
        (t
         (exit-with-error "usage error" +usage-error+ 2)))
      (let ((text
              (handler-case
                  (read-utf8-file path)
                (error ()
                  (exit-with-error
                   "io error" (format nil "unable to read '~A'" path) 3)))))
        (handler-case
            (let* ((graph (parse-graph text))
                   (result
                     (if (string= command "analyze")
                         (analyze-graph graph)
                         (affected-tasks graph query))))
              (write-string result)
              (finish-output)
              (sb-ext:exit :code 0))
          (buildgraph-input-error (condition)
            (exit-with-error
             "input error"
             (buildgraph-input-error-message condition)
             4)))))))
