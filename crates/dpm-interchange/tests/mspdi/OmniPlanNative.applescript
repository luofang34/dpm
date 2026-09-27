-- Second step of authoring omniplan-native.xml: Omni Automation does not expose lead times or
-- constraint dates, so they are set through OmniPlan's AppleScript dictionary. The argument is the
-- index of the new document created by OmniPlanNative.js (1 when it is frontmost).
on leadOf(successorTask, prerequisiteName, secs)
	tell application id "com.omnigroup.OmniPlan4.MacAppStore"
		repeat with p in (every prerequisite of successorTask)
			if name of prerequisite task of p is prerequisiteName then
				set lead time of p to secs
				return
			end if
		end repeat
	end tell
	error "no prerequisite " & prerequisiteName
end leadOf

on dateAt(y, m, d, h)
	set c to current date
	set year of c to y
	set day of c to 1
	set month of c to m
	set day of c to d
	set time of c to h * hours
	return c
end dateAt

on run argv
	with timeout of 50 seconds
		tell application id "com.omnigroup.OmniPlan4.MacAppStore"
			tell document ((item 1 of argv) as integer)
				-- A new document starts with a placeholder task and template resources.
				delete (every task whose name is "Task 1")
				repeat with n in {"Staff 1", "Staff 2", "Equipment & Material", "Conference Room 1"}
					delete (every resource whose name is (n as text))
				end repeat
				set spec to first task whose name is "编写规格说明"
				set review to first task whose name is "审查规格"
				set impl to first task whose name is "实现功能"
				set docs to first task whose name is "编写文档"
				set tst to first task whose name is "集成测试"
				set rel to first task whose name is "发布 Release"
			end tell
			my leadOf(review, "编写规格说明", 7200)
			my leadOf(impl, "设计批准", 28800)
			my leadOf(docs, "实现功能", 14400)
			my leadOf(tst, "实现功能", -10800)
			my leadOf(tst, "编写文档", 21600)
			my leadOf(rel, "集成测试", -7200)
			set starting constraint date of spec to my dateAt(2026, October, 5, 9)
			set ending constraint date of tst to my dateAt(2026, October, 30, 17)
		end tell
	end timeout
	return "tuned"
end run
