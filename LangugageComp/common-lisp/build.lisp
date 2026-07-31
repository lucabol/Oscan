(let* ((directory (make-pathname :name nil :type nil :defaults *load-truename*))
       (source (merge-pathnames "main.lisp" directory))
       (output
         (merge-pathnames
          #+win32 "../.build/common-lisp/buildgraph.exe"
          #-win32 "../.build/common-lisp/buildgraph"
          directory)))
  (ensure-directories-exist output)
  (load source)
  (sb-ext:save-lisp-and-die
   (namestring output)
   :toplevel (symbol-function (find-symbol "MAIN" "BUILDGRAPH"))
   :executable t
   :save-runtime-options t))
