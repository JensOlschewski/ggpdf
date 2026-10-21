on run
    set selectedFiles to choose file with prompt "Open PDFs in PDF Preview" of type {"com.adobe.pdf"} with multiple selections allowed
    open selectedFiles
end run

on open selectedFiles
    set previewScript to "__GGPDF_PREVIEW_SCRIPT__"
    set launchCommand to "/usr/bin/python3 " & quoted form of previewScript
    repeat with selectedFile in selectedFiles
        set launchCommand to launchCommand & " " & quoted form of (POSIX path of selectedFile)
    end repeat
    try
        do shell script launchCommand
    on error errorMessage
        display alert "PDF Preview could not open the document" message errorMessage as critical
    end try
end open
